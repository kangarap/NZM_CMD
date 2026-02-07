use crate::human::HumanDriver;
use crate::nav::NavEngine;
use regex::Regex;
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

// ==========================================
// 1. 数据结构协议
// ==========================================

// ==========================================
// 新增：预备阶段动作定义
// ==========================================
#[derive(Deserialize, Debug, Clone)]
#[serde(tag = "type")]
pub enum PrepAction {
    KeyDown { key: char },
    KeyUpAll,
    Wait { ms: u64 },
    Log { msg: String },
}

// 辅助函数：将字符转换为 HID 键码 (为了不依赖 human.rs 的私有方法，这里简单实现一个)
fn get_hid_code(c: char) -> u8 {
    match c.to_ascii_lowercase() {
        'a'..='z' => c.to_ascii_lowercase() as u8 - b'a' + 0x04,
        '0'..='9' => c as u8 - b'1' + 0x1E, // 简略处理
        ' ' => 0x2C,
        _ => 0,
    }
}
#[derive(Deserialize, Debug, Clone)]
#[serde(tag = "type")] // JSON 中使用 "type": "Click" 来区分
pub enum InitAction {
    Move {
        x: u16,
        y: u16,
    },
    Click {
        #[serde(default)]
        left: bool,
        #[serde(default)]
        right: bool,
        #[serde(default)]
        hold_ms: u64,
    },
    Key {
        char: char,
    },
    Wait {
        ms: u64,
    },
    Log {
        msg: String,
    },
}

#[derive(Debug, Clone)]
pub struct TDConfig {
    pub hud_check_rect: [i32; 4],
    pub hud_wave_loop_rect: [i32; 4],
    pub safe_zone: [i32; 4],
    pub screen_width: f32,
    pub screen_height: f32,
}

impl Default for TDConfig {
    fn default() -> Self {
        Self {
            hud_check_rect: [262, 16, 389, 97],
            hud_wave_loop_rect: [350, 288, 582, 362],
            safe_zone: [200, 200, 1720, 880],
            screen_width: 1920.0,
            screen_height: 1080.0,
        }
    }
}

#[derive(Deserialize, Debug, Clone)]
pub struct TrapConfigItem {
    pub name: String,
    #[serde(default)]
    pub b_type: String,
    #[serde(default)]
    pub grid_index: [i32; 2],
}

#[derive(Deserialize, Debug, Clone)]
pub struct MapMeta {
    pub grid_pixel_size: f32,
    pub offset_x: f32,
    pub offset_y: f32,
    pub bottom: f32,
    #[serde(default)]
    pub prep_actions: Vec<PrepAction>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct BuildingExport {
    pub uid: usize,
    pub name: String,
    pub grid_x: usize,
    pub grid_y: usize,
    pub width: usize,
    pub height: usize,
    #[serde(default)]
    pub wave_num: i32,
    #[serde(default)]
    pub is_late: bool,
}

#[derive(Deserialize, Debug, Clone)]
pub struct UpgradeEvent {
    pub building_name: String,
    pub wave_num: i32,
    pub is_late: bool,
}

#[derive(Deserialize, Debug, Clone)]
pub struct DemolishEvent {
    pub uid: usize,
    pub name: String,
    pub grid_x: usize,
    pub grid_y: usize,
    pub width: usize,
    pub height: usize,
    pub wave_num: i32,
    pub is_late: bool,
}

#[derive(Deserialize, Debug, Clone)]
pub struct MapTerrainExport {
    pub map_name: String,
    pub meta: MapMeta,
}

#[derive(Deserialize, Debug, Clone)]
pub struct MapBuildingsExport {
    pub map_name: String,
    pub buildings: Vec<BuildingExport>,
    #[serde(default)]
    pub upgrades: Vec<UpgradeEvent>,
    #[serde(default)]
    pub demolishes: Vec<DemolishEvent>,
}

#[derive(Debug, Default)]
pub struct WaveStatus {
    pub current_wave: i32,
}

struct TaskWithPos<T> {
    data: T,
    map_y: f32,
    map_x: f32,
}

#[derive(Clone)]
enum TaskAction {
    Demolish(DemolishEvent),
    Place(BuildingExport),
    Upgrade(UpgradeEvent),
}

#[derive(Clone)]
struct ScheduledTask {
    action: TaskAction,
    map_y: f32,
    map_x: f32,
    priority: u8,
}

// ==========================================
// 2. 塔防模块实现
// ==========================================
pub struct TowerDefenseApp {
    driver: Arc<Mutex<HumanDriver>>,
    nav: Arc<NavEngine>,
    config: TDConfig,
    map_meta: Option<MapMeta>,

    strategy_buildings: Vec<BuildingExport>,
    strategy_upgrades: Vec<UpgradeEvent>,
    strategy_demolishes: Vec<DemolishEvent>,

    placed_uids: HashSet<usize>,
    completed_upgrade_keys: HashSet<String>,
    completed_demolish_uids: HashSet<usize>,

    last_confirmed_wave: i32,
    last_wave_change_time: Instant,

    trap_lookup: HashMap<String, TrapConfigItem>,
    active_loadout: Vec<String>,

    camera_offset_y: f32,
    move_speed: f32,
}

impl TowerDefenseApp {
    pub fn new(driver: Arc<Mutex<HumanDriver>>, nav: Arc<NavEngine>) -> Self {
        Self {
            driver,
            nav,
            config: TDConfig::default(),
            map_meta: None,
            strategy_buildings: Vec::new(),
            strategy_upgrades: Vec::new(),
            strategy_demolishes: Vec::new(),
            placed_uids: HashSet::new(),
            completed_upgrade_keys: HashSet::new(),
            completed_demolish_uids: HashSet::new(),
            last_confirmed_wave: 0,
            last_wave_change_time: Instant::now(),
            trap_lookup: HashMap::new(),
            active_loadout: Vec::new(),
            camera_offset_y: 0.0,
            move_speed: 720.0,
        }
    }

    pub fn load_strategy(&mut self, path: &str) {
        if let Ok(c) = fs::read_to_string(path) {
            if let Ok(data) = serde_json::from_str::<MapBuildingsExport>(&c) {
                self.strategy_buildings = data.buildings;
                self.strategy_upgrades = data.upgrades;
                self.strategy_demolishes = data.demolishes;
                println!(
                    "🏗️ 策略加载成功: 建{} | 升{} | 拆{}",
                    self.strategy_buildings.len(),
                    self.strategy_upgrades.len(),
                    self.strategy_demolishes.len()
                );
            } else {
                println!("❌ 策略 JSON 解析失败");
            }
        }
    }

    pub fn recognize_wave_status(&self, rect: [i32; 4], use_tab: bool) -> Option<WaveStatus> {
        const KEY_TAB: u8 = 0x2B;
        if use_tab {
            if let Ok(driver) = self.driver.lock() {
                if let Ok(mut dev) = driver.device.lock() {
                    dev.key_down(KEY_TAB, 0);
                }
            }
            thread::sleep(Duration::from_millis(500));
        }

        let text: String = self.nav.ocr_area(rect);

        if use_tab {
            if let Ok(driver) = self.driver.lock() {
                if let Ok(mut dev) = driver.device.lock() {
                    dev.key_up();
                }
            }
            thread::sleep(Duration::from_millis(500));
            if let Ok(driver) = self.driver.lock() {
                if let Ok(mut dev) = driver.device.lock() {
                    dev.key_down(KEY_TAB, 0);
                }
            }
            thread::sleep(Duration::from_millis(100));
            if let Ok(driver) = self.driver.lock() {
                if let Ok(mut dev) = driver.device.lock() {
                    dev.key_up();
                }
            }
        }

        if text.is_empty() {
            return None;
        }

        println!("🔍 [OCR Debug] 原始文本: 「{}」 (Mode: {})", text.trim(), if use_tab { "TAB" } else { "HUD" });

        let val = if use_tab {
            let re = Regex::new(r"(\d+)[/\dSI日]+.*波次").ok()?;
            re.captures(&text).and_then(|caps| {
                let num = caps.get(1)?.as_str().parse::<i32>().ok()?;
                println!("✅ [OCR Match] TAB 模式匹配成功: 第 {} 波", num);
                Some(num)
            })?
        } else {
            let re = Regex::new(r"波次\s*(\d+)").ok()?;
            re.captures(&text).and_then(|caps| {
                let num = caps.get(1)?.as_str().parse::<i32>().ok()?;
                println!("✅ [OCR Match] HUD 模式匹配成功: 第 {} 波", num);
                Some(num)
            })?
        };
        Some(WaveStatus { current_wave: val })
    }

    fn validate_wave_transition(&mut self, detected_wave: i32) -> bool {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_wave_change_time).as_secs();
        let is_next_wave = detected_wave == self.last_confirmed_wave + 1;
        let is_long_enough = elapsed >= 60 || self.last_confirmed_wave == 0;
        if is_next_wave && is_long_enough {
            println!(
                "✅ [Monitor] 新波次: {} -> {}",
                self.last_confirmed_wave, detected_wave
            );
            self.last_confirmed_wave = detected_wave;
            self.last_wave_change_time = now;
            true
        } else {
            false
        }
    }

    // 🔥 新增：辅助函数，判断任务是否都在当前视野安全区内
    fn are_tasks_in_current_view(&self, tasks: &[ScheduledTask]) -> bool {
        let [_, sz_y1, _, sz_y2] = self.config.safe_zone;
        
        // 当前屏幕顶部在地图上的逻辑坐标
        let view_top = self.camera_offset_y;
        
        // 安全区的绝对地图坐标范围
        let safe_map_top = view_top + sz_y1 as f32;
        let safe_map_bottom = view_top + sz_y2 as f32;

        for task in tasks {
            if task.map_y < safe_map_top || task.map_y > safe_map_bottom {
                return false;
            }
        }
        true
    }
pub fn execute_wave_phase(&mut self, wave: i32, is_late: bool) {
        let phase_name = if is_late { "后期" } else { "前期" };
        println!("🚀 优化执行第 {} 波 [{}] (拆除优先模式)...", wave, phase_name);

        // 1. 分类收集任务
        let mut demolish_tasks = Vec::new();
        let mut build_upgrade_tasks = Vec::new();

        // 收集拆除任务 (Priority 0)
        for d in self.strategy_demolishes.iter().filter(|d| {
            d.wave_num == wave && d.is_late == is_late && !self.completed_demolish_uids.contains(&d.uid)
        }) {
            if let Some((px, py)) = self.get_absolute_map_pixel(d.grid_x, d.grid_y, d.width, d.height) {
                demolish_tasks.push(ScheduledTask {
                    action: TaskAction::Demolish(d.clone()),
                    map_y: py, map_x: px, priority: 0,
                });
            }
        }

        // 收集建造任务 (Priority 1)
        for b in self.strategy_buildings.iter().filter(|b| {
            b.wave_num == wave && b.is_late == is_late && !self.placed_uids.contains(&b.uid)
        }) {
            if let Some((px, py)) = self.get_absolute_map_pixel(b.grid_x, b.grid_y, b.width, b.height) {
                build_upgrade_tasks.push(ScheduledTask {
                    action: TaskAction::Place(b.clone()),
                    map_y: py, map_x: px, priority: 1,
                });
            }
        }

        // 收集升级任务 (Priority 2)
        for u in self.strategy_upgrades.iter().filter(|u| u.wave_num == wave && u.is_late == is_late) {
            let key = format!("{}-{}-{}", u.building_name, u.wave_num, u.is_late);
            if !self.completed_upgrade_keys.contains(&key) {
                build_upgrade_tasks.push(ScheduledTask {
                    action: TaskAction::Upgrade(u.clone()),
                    map_y: 0.0, map_x: 0.0, priority: 2,
                });
            }
        }

        if demolish_tasks.is_empty() && build_upgrade_tasks.is_empty() {
            return;
        }

        // --- 第一阶段：优先执行所有拆除任务 ---
        if !demolish_tasks.is_empty() {
            println!("🧹 [Step 1] 正在执行全图拆除任务 ({}个)...", demolish_tasks.len());
            self.dispatch_tasks_by_region(demolish_tasks);
        }

        // --- 第二阶段：执行建造和升级任务 ---
        if !build_upgrade_tasks.is_empty() {
            println!("🏗️ [Step 2] 正在执行建造与升级任务 ({}个)...", build_upgrade_tasks.len());
            // 确保建造内部依然遵循 Priority (先建后升)
            build_upgrade_tasks.sort_by(|a, b| a.priority.cmp(&b.priority));
            self.dispatch_tasks_by_region(build_upgrade_tasks);
        }
    }

    /// 辅助函数：将一组任务按区域执行（包含智能归零逻辑）
    fn dispatch_tasks_by_region(&mut self, tasks: Vec<ScheduledTask>) {
        let meta = self.map_meta.as_ref().unwrap();
        let map_h = meta.bottom;
        let screen_h = self.config.screen_height;
        let mid_point = (map_h - screen_h) / 2.0;

        // 分区：上半区 vs 下半区
        let (mut upper, mut lower): (Vec<_>, Vec<_>) = tasks
            .into_iter()
            .partition(|t| t.map_y <= mid_point + screen_h / 2.0);

        // 处理上半区
        if !upper.is_empty() {
            upper.sort_by(|a, b| a.map_y.partial_cmp(&b.map_y).unwrap().then(a.priority.cmp(&b.priority)));
            if self.are_tasks_in_current_view(&upper) {
                println!("✨ 上半区任务在视野内，直接执行");
                self.process_task_batch(upper, false);
            } else {
                self.align_camera_to_edge(true);
                self.process_task_batch(upper, true);
            }
        }

        // 处理下半区
        if !lower.is_empty() {
            lower.sort_by(|a, b| b.map_y.partial_cmp(&a.map_y).unwrap().then(a.priority.cmp(&b.priority)));
            if self.are_tasks_in_current_view(&lower) {
                println!("✨ 下半区任务在视野内，直接执行");
                self.process_task_batch(lower, false);
            } else {
                self.align_camera_to_edge(false);
                self.process_task_batch(lower, true);
            }
        }
    }
    fn process_task_batch(&mut self, tasks: Vec<ScheduledTask>, force_initial_refresh: bool) {
        let mut last_build_key: Option<char> = None;
        let mut is_first_task = true;

        for task in tasks {
            if let TaskAction::Upgrade(u) = &task.action {
                self.execute_single_upgrade(u);
                continue;
            }

            // 计算是否因为距离变动导致了“屏幕移动”
            let mut screen_moved = self.smart_move_camera(task.map_y);

            // 如果是本批次的第一个任务，且外部要求强制刷新（因为刚归零过），
            // 那么强制认为 screen_moved = true，从而触发 perform_build_action 中的“三连击”
            if is_first_task && force_initial_refresh {
                screen_moved = true;
                is_first_task = false;
            }

            match &task.action {
                TaskAction::Demolish(d) => {
                    self.perform_demolish_action(task.map_x, task.map_y, d.uid)
                }
                TaskAction::Place(b) => self.perform_build_action(
                    &mut last_build_key,
                    screen_moved,
                    task.map_x,
                    task.map_y,
                    &b.name,
                    b.uid,
                ),
                _ => {}
            }
        }
    }

    fn perform_demolish_action(&mut self, map_x: f32, map_y: f32, uid: usize) {
        let [sz_x1, sz_y1, sz_x2, sz_y2] = self.config.safe_zone;
        let screen_x = (map_x - 0.0).clamp(sz_x1 as f32, sz_x2 as f32);
        let screen_y = (map_y - self.camera_offset_y).clamp(sz_y1 as f32, sz_y2 as f32);

        if let Ok(mut driver) = self.driver.lock() {
            driver.move_to_humanly(screen_x as u16, screen_y as u16, 0.4);
            driver.click_humanly(true, false, 0);
            thread::sleep(Duration::from_millis(150));
            driver.key_click('e');
        }
        self.completed_demolish_uids.insert(uid);
        thread::sleep(Duration::from_millis(300));
    }

    fn perform_build_action(
        &mut self,
        last_key: &mut Option<char>,
        screen_moved: bool,
        map_x: f32,
        map_y: f32,
        name: &str,
        uid: usize,
    ) {
        let [sz_x1, sz_y1, sz_x2, sz_y2] = self.config.safe_zone;
        let screen_x = (map_x - 0.0).clamp(sz_x1 as f32, sz_x2 as f32);
        let screen_y = (map_y - self.camera_offset_y).clamp(sz_y1 as f32, sz_y2 as f32);
        let key = self.get_trap_key(name);

        if let Ok(mut d) = self.driver.lock() {
            d.move_to_humanly(screen_x as u16, screen_y as u16, 0.35);

            // 策略执行：只有在屏幕动过（或刚归零过）时才进行三连击
            if screen_moved {
                let swap_key = if key == '4' { '5' } else { '4' };
                d.key_click(key);
                thread::sleep(Duration::from_millis(50));
                d.key_click(swap_key);
                thread::sleep(Duration::from_millis(50));
                d.key_click(key);
                thread::sleep(Duration::from_millis(150));
                *last_key = Some(key);
            } else if Some(key) != *last_key {
                // 原地换塔：只点一次
                d.key_click(key);
                *last_key = Some(key);
                thread::sleep(Duration::from_millis(150));
            }

            d.double_click_humanly(true, false, 200);
        }
        self.placed_uids.insert(uid);
        thread::sleep(Duration::from_millis(250));
    }

    fn execute_single_upgrade(&mut self, u: &UpgradeEvent) {
        let key = self.get_trap_key(&u.building_name);
        if let Ok(mut d) = self.driver.lock() {
            println!("   -> 长按 '{}' (800ms) 以升级: {}", key, u.building_name);
            d.key_hold(key, 1500);
        }
        let key_str = format!("{}-{}-{}", u.building_name, u.wave_num, u.is_late);
        self.completed_upgrade_keys.insert(key_str);
        thread::sleep(Duration::from_millis(400));
    }

    fn align_camera_to_edge(&mut self, top: bool) {
        let meta = self.map_meta.as_ref().unwrap();
        let max_scroll_y = (meta.bottom - self.config.screen_height).max(0.0);

        if let Ok(mut human) = self.driver.lock() {
            let key = if top { 'w' } else { 's' };
            println!("🔄 强制归零: {}", if top { "顶部" } else { "底部" });
            human.key_hold(key, 2500);
        }
        self.camera_offset_y = if top { 0.0 } else { max_scroll_y };
        thread::sleep(Duration::from_millis(500));
    }

    // 🔥 新增：像素级滚动封装函数
    fn scroll_camera_by_pixels(&self, direction: char, pixels: f32, time_resolution_ms: u64) -> f32 {
        if pixels < 10.0 { return 0.0; }

        let raw_ms = (pixels / self.move_speed * 1000.0) as u64;
        
        // 量子化取整
        let units = (raw_ms + time_resolution_ms / 2) / time_resolution_ms;
        let final_ms = units.max(1) * time_resolution_ms;

        if let Ok(mut human) = self.driver.lock() {
            // println!("📷 滚动: {:.1}px -> {}ms", pixels, final_ms);
            human.key_hold(direction, final_ms);
        }

        // 返回实际移动距离
        (final_ms as f32 / 1000.0) * self.move_speed
    }

    // 返回 true 表示确实进行了物理移动
    fn smart_move_camera(&mut self, target_map_y: f32) -> bool {
        let [_, z_y1, _, z_y2] = self.config.safe_zone;
        let screen_h = self.config.screen_height;
        let safe_center_screen_y = (z_y1 + z_y2) as f32 / 2.0;
        let max_scroll_y = (self.map_meta.as_ref().unwrap().bottom - screen_h).max(0.0);

        let ideal_cam_y = (target_map_y - safe_center_screen_y).clamp(0.0, max_scroll_y);
        let delta = ideal_cam_y - self.camera_offset_y;

        // 小于 30 像素不移动
        if delta.abs() < 30.0 {
            return false;
        }

        let mid_scroll = max_scroll_y / 2.0;
        const SCROLL_RES: u64 = 100; // 时间分辨率 100ms

        if ideal_cam_y <= mid_scroll {
            // 归零到顶部 (0)
            self.align_camera_to_edge(true);
            self.camera_offset_y = 0.0;

            // 向下微调
            if ideal_cam_y > 10.0 {
                let moved = self.scroll_camera_by_pixels('s', ideal_cam_y, SCROLL_RES);
                self.camera_offset_y += moved;
            }
        } else {
            // 归零到底部
            self.align_camera_to_edge(false);
            self.camera_offset_y = max_scroll_y;

            // 向上微调
            let dist_up = max_scroll_y - ideal_cam_y;
            if dist_up > 10.0 {
                let moved = self.scroll_camera_by_pixels('w', dist_up, SCROLL_RES);
                self.camera_offset_y -= moved;
            }
        }

        thread::sleep(Duration::from_millis(200));
        true
    }

    pub fn load_map_terrain(&mut self, path: &str) {
        if let Ok(c) = fs::read_to_string(path) {
            if let Ok(data) = serde_json::from_str::<MapTerrainExport>(&c) {
                self.map_meta = Some(data.meta);
            }
        }
    }

    pub fn load_trap_config(&mut self, json_path: &str) {
        if let Ok(c) = fs::read_to_string(json_path) {
            if let Ok(items) = serde_json::from_str::<Vec<TrapConfigItem>>(&c) {
                for item in items {
                    self.trap_lookup.insert(item.name.clone(), item);
                }
            }
        }
    }

    pub fn setup_view(&mut self) {
        println!("🔭 对齐左上角边界...");
        if let Ok(mut human) = self.driver.lock() {
            human.key_click('o');
            thread::sleep(Duration::from_secs(2));
            for _ in 1..=4 {
                for _ in 0..10 {
                    human.mouse_scroll(-120);
                    thread::sleep(Duration::from_millis(30));
                }
                thread::sleep(Duration::from_millis(100));
            }
            for _ in 1..=2 {
                human.key_hold('w', 200);
                thread::sleep(Duration::from_millis(50));
                human.key_hold('a', 200);
                thread::sleep(Duration::from_millis(50));
            }
            human.key_hold('w', 200);
            human.key_hold('a', 200);
        }
        self.camera_offset_y = 0.0;
    }

pub fn execute_prep_logic(&self) {
        println!("🔧 执行赛前准备...");

        // 1. 执行 MapMeta 中配置的战术动作 (如助跑跳)
        if let Some(meta) = &self.map_meta {
            if !meta.prep_actions.is_empty() {
                println!("   -> 加载自定义战术动作 ({} 步)", meta.prep_actions.len());
                if let Ok(mut human) = self.driver.lock() {
                    if let Ok(mut dev) = human.device.lock() {
                        for action in &meta.prep_actions {
                            match action {
                                PrepAction::KeyDown { key } => {
                                    let code = get_hid_code(*key);
                                    if code != 0 { dev.key_down(code, 0); }
                                }
                                PrepAction::KeyUpAll => {
                                    dev.key_up();
                                }
                                PrepAction::Wait { ms } => {
                                    drop(dev); // 释放锁以允许心跳
                                    thread::sleep(Duration::from_millis(*ms));
                                    dev = human.device.lock().unwrap(); // 重新获取锁
                                }
                                PrepAction::Log { msg } => {
                                    println!("   [Prep] {}", msg);
                                }
                            }
                        }
                        dev.key_up(); // 确保结束后松开按键
                    }
                }
            }
        }

        // 2. 打开菜单
        if let Ok(mut human) = self.driver.lock() {
            human.key_click('n');
            thread::sleep(Duration::from_millis(500));
        }

        // 3. 选塔 (使用内部 active_loadout)
        self.select_loadout();

        // 4. 关闭菜单
        if let Ok(mut human) = self.driver.lock() {
            human.key_click('n');
            thread::sleep(Duration::from_millis(500));
        }
    }
    pub fn select_loadout(&self) {
        const GRID_START_X: i32 = 520;
        const GRID_START_Y: i32 = 330;
        const GRID_STEP_X: i32 = 170;
        const GRID_STEP_Y: i32 = 205;

        for name in self.active_loadout.iter().take(4) {
            if let Some(config) = self.trap_lookup.get(name) {
                // 1. 根据 b_type 切换标签
                let (tab_x, tab_y) = match config.b_type.as_str() {
                    "Wall" => (172, 375),    // ⚠️ TODO: 请确认墙面标签的 Y 坐标
                    "Ceiling" => (172, 462), // ⚠️ TODO: 请确认天花板标签的 Y 坐标
                    _ => (172, 294),         // 默认为地面 Floor
                };

                if let Ok(mut d) = self.driver.lock() {
                    // 点击分类标签
                    d.move_to_humanly(tab_x, tab_y, 0.4);
                    d.click_humanly(true, false, 0);
                    thread::sleep(Duration::from_millis(350));

                    // 2. 计算网格坐标
                    let col = config.grid_index[0];
                    let row = config.grid_index[1];
                    let target_x = GRID_START_X + col * GRID_STEP_X;
                    let target_y = GRID_START_Y + row * GRID_STEP_Y;

                    // 点击具体陷阱
                    d.move_to_humanly(target_x as u16, target_y as u16, 0.4);
                    d.click_humanly(true, false, 0);
                }
                thread::sleep(Duration::from_millis(400));
            } else {
                println!("⚠️ [Config Error] 未找到陷阱配置: {}", name);
            }
        }
    }

    fn get_absolute_map_pixel(
        &self,
        gx: usize,
        gy: usize,
        w: usize,
        h: usize,
    ) -> Option<(f32, f32)> {
        let meta = self.map_meta.as_ref()?;
        let sx = meta.offset_x + ((gx as f32 + w as f32 / 2.0) * meta.grid_pixel_size);
        let sy = meta.offset_y + ((gy as f32 + h as f32 / 2.0) * meta.grid_pixel_size);
        Some((sx, sy))
    }

    fn get_trap_key(&self, name: &str) -> char {
        let index = self
            .active_loadout
            .iter()
            .position(|t| t == name)
            .unwrap_or(0);
        match index {
            0 => '4',
            1 => '5',
            2 => '6',
            3 => '7',
            _ => '1',
        }
    }

pub fn run(&mut self, terrain_p: &str, strategy_p: &str, trap_p: &str) {
        // 1. 加载所有配置
        self.load_map_terrain(terrain_p);
        self.load_trap_config(trap_p); // 先加载陷阱库
        self.load_strategy(strategy_p); // 再加载策略

        // 2. ✨ 自动从策略中提取需要携带的陷阱
        let mut seen = HashSet::new();
        let mut derived_loadout = Vec::new();

        // 收集建造任务中的塔
        for b in &self.strategy_buildings {
            if !seen.contains(&b.name) && self.trap_lookup.contains_key(&b.name) {
                seen.insert(b.name.clone());
                derived_loadout.push(b.name.clone());
            }
        }
        // 收集升级任务中的塔
        for u in &self.strategy_upgrades {
            if !seen.contains(&u.building_name) && self.trap_lookup.contains_key(&u.building_name) {
                seen.insert(u.building_name.clone());
                derived_loadout.push(u.building_name.clone());
            }
        }

        if derived_loadout.is_empty() {
             println!("⚠️ 警告: 策略中未发现已知陷阱，装备栏将为空！");
        } else {
             println!("📋 自动分析策略，生成装备列表: {:?}", derived_loadout);
        }
        self.active_loadout = derived_loadout;

        // 3. 进入游戏逻辑
        if let Ok(mut human) = self.driver.lock() {
            println!("👆 点击游戏入口...");
            human.move_to_humanly(1700, 950, 0.5);
            human.click_humanly(true, false, 0);
            human.move_to_humanly(1110, 670, 0.5);
            human.click_humanly(true, false, 0);
        }

        println!("⏳ 等待战斗开始...");
        loop {
            if let Some(status) = self.recognize_wave_status(self.config.hud_check_rect, false) {
                if status.current_wave > 0 {
                    println!("🎮 战斗开始! 初始波次: {}", status.current_wave);
                    self.last_wave_change_time = Instant::now();
                    break;
                }
            }
            thread::sleep(Duration::from_millis(1000));
        }

        // 4. 执行赛前准备 (无参调用)
        self.execute_prep_logic();
        self.setup_view();

        println!("🤖 自动化监控中...");
        loop {
            if let Some(status) = self.recognize_wave_status(self.config.hud_wave_loop_rect, true) {
                if self.validate_wave_transition(status.current_wave) {
                    let current_wave = status.current_wave;
                    self.execute_wave_phase(current_wave, false);
                    println!("🔔 波次 {} 前期完成，按 G 开战", current_wave);
                    if let Ok(mut d) = self.driver.lock() {
                        d.key_click('g');
                    }
                    thread::sleep(Duration::from_secs(1));
                    self.execute_wave_phase(current_wave, true);
                }
            }
            thread::sleep(Duration::from_millis(10000));
        }
    }
}