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
    pub select_pos: [i32; 2],
}

#[derive(Deserialize, Debug, Clone)]
pub struct MapMeta {
    pub grid_pixel_size: f32,
    pub offset_x: f32,
    pub offset_y: f32,
    pub bottom: f32,
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
        println!("🚀 优化执行第 {} 波 [{}]...", wave, phase_name);

        let mut tasks: Vec<ScheduledTask> = Vec::new();

        for d in self.strategy_demolishes.iter().filter(|d| {
            d.wave_num == wave
                && d.is_late == is_late
                && !self.completed_demolish_uids.contains(&d.uid)
        }) {
            if let Some((px, py)) =
                self.get_absolute_map_pixel(d.grid_x, d.grid_y, d.width, d.height)
            {
                tasks.push(ScheduledTask {
                    action: TaskAction::Demolish(d.clone()),
                    map_y: py,
                    map_x: px,
                    priority: 0,
                });
            }
        }
        for b in self.strategy_buildings.iter().filter(|b| {
            b.wave_num == wave && b.is_late == is_late && !self.placed_uids.contains(&b.uid)
        }) {
            if let Some((px, py)) =
                self.get_absolute_map_pixel(b.grid_x, b.grid_y, b.width, b.height)
            {
                tasks.push(ScheduledTask {
                    action: TaskAction::Place(b.clone()),
                    map_y: py,
                    map_x: px,
                    priority: 1,
                });
            }
        }
        for u in self
            .strategy_upgrades
            .iter()
            .filter(|u| u.wave_num == wave && u.is_late == is_late)
        {
            let key = format!("{}-{}-{}", u.building_name, u.wave_num, u.is_late);
            if !self.completed_upgrade_keys.contains(&key) {
                tasks.push(ScheduledTask {
                    action: TaskAction::Upgrade(u.clone()),
                    map_y: 0.0,
                    map_x: 0.0,
                    priority: 2,
                });
            }
        }

        if tasks.is_empty() {
            return;
        }

        println!("📊 [Debug] 第 {} 波任务收集: 拆除 {}, 建造 {}, 升级 {}", 
            wave, 
            tasks.iter().filter(|t| matches!(t.action, TaskAction::Demolish(_))).count(),
            tasks.iter().filter(|t| matches!(t.action, TaskAction::Place(_))).count(),
            tasks.iter().filter(|t| matches!(t.action, TaskAction::Upgrade(_))).count()
        );

        let meta = self.map_meta.as_ref().unwrap();
        let map_h = meta.bottom;
        let screen_h = self.config.screen_height;
        let mid_point = (map_h - screen_h) / 2.0;

        let (mut upper_tasks, mut lower_tasks): (Vec<_>, Vec<_>) = tasks
            .into_iter()
            .partition(|t| t.map_y <= mid_point + screen_h / 2.0);


        println!("📊 [Debug] 分区情况: 上半区 {} 个, 下半区 {} 个", upper_tasks.len(), lower_tasks.len());

        // --- 上半区处理逻辑 ---
        if !upper_tasks.is_empty() {
            println!("⬆️ 准备执行上半区任务...");
            upper_tasks.sort_by(|a, b| {
                a.map_y
                    .partial_cmp(&b.map_y)
                    .unwrap()
                    .then(a.priority.cmp(&b.priority))
            });

            // 🔥 智能归零判断
            if self.are_tasks_in_current_view(&upper_tasks) {
                println!("✨ [Smart] 上半区任务均在当前视野内，跳过归零！");
                self.process_task_batch(upper_tasks, false);
            } else {
                println!("🔄 [Reset] 任务超出视野，执行顶部归零...");
                self.align_camera_to_edge(true);
                self.process_task_batch(upper_tasks, true);
            }
        }

        // --- 下半区处理逻辑 ---
        if !lower_tasks.is_empty() {
            println!("⬇️ 准备执行下半区任务...");
            lower_tasks.sort_by(|a, b| {
                b.map_y
                    .partial_cmp(&a.map_y)
                    .unwrap()
                    .then(a.priority.cmp(&b.priority))
            });

            // 🔥 智能归零判断
            if self.are_tasks_in_current_view(&lower_tasks) {
                println!("✨ [Smart] 下半区任务均在当前视野内，跳过归零！");
                self.process_task_batch(lower_tasks, false);
            } else {
                println!("🔄 [Reset] 任务超出视野，执行底部归零...");
                self.align_camera_to_edge(false);
                self.process_task_batch(lower_tasks, true);
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

    pub fn execute_prep_logic(&self, loadout: &[&str]) {
        println!("🔧 执行赛前准备...");

        if let Ok(mut human) = self.driver.lock() {
            // W + Space 组合键
            if let Ok(mut dev) = human.device.lock() {
                // (1) 按下 W
                dev.key_down(0x1A, 0);
            }
            thread::sleep(Duration::from_millis(1000)); // 助跑时间

            if let Ok(mut dev) = human.device.lock() {
                // (2) 按下 Space
                dev.key_down(0x2C, 0);
            }
            thread::sleep(Duration::from_millis(100)); // 起跳判定时间

            if let Ok(mut dev) = human.device.lock() {
                // (3) 松开所有
                dev.key_up();
            }
            
            // 为了稳妥，再做一遍
             if let Ok(mut dev) = human.device.lock() {
                dev.key_down(0x1A, 0);
            }
            thread::sleep(Duration::from_millis(200)); 

            if let Ok(mut dev) = human.device.lock() {
                dev.key_down(0x2C, 0);
            }
            thread::sleep(Duration::from_millis(100)); 

            if let Ok(mut dev) = human.device.lock() {
                dev.key_up();
            }
            println!("   -> 执行战术动作: W + Space");
        }

        if let Ok(mut human) = self.driver.lock() {
            human.key_click('n');
            thread::sleep(Duration::from_millis(500));
            human.move_to_humanly(212, 294, 0.5);
            human.click_humanly(true, false, 0);
        }
        self.select_loadout(loadout);
        if let Ok(mut human) = self.driver.lock() {
            human.key_click('n');
            thread::sleep(Duration::from_millis(500));
        }
    }

    pub fn select_loadout(&self, tower_names: &[&str]) {
        for name in tower_names.iter().take(4) {
            if let Some(config) = self.trap_lookup.get(*name) {
                let [x, y] = config.select_pos;
                if let Ok(mut d) = self.driver.lock() {
                    d.move_to_humanly(x as u16, y as u16, 0.5);
                    d.click_humanly(true, false, 0);
                }
                thread::sleep(Duration::from_millis(400));
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

    pub fn run(&mut self, terrain_p: &str, strategy_p: &str, trap_p: &str, loadout: &[&str]) {
        self.active_loadout = loadout.iter().map(|&s| s.to_string()).collect();
        self.load_map_terrain(terrain_p);
        self.load_strategy(strategy_p);
        self.load_trap_config(trap_p);

        if let Ok(mut human) = self.driver.lock() {
            println!("👆 点击游戏入口...");
            human.move_to_humanly(1700, 950, 0.5);
            human.click_humanly(true, false, 0);
            human.move_to_humanly(1110, 670, 0.5);
            human.click_humanly(true, false, 0);
        }

        println!("⏳ 等待战斗开始...");
        loop {
            // 初始阶段：不需要 TAB，用旧正则
            if let Some(status) = self.recognize_wave_status(self.config.hud_check_rect, false) {
                if status.current_wave > 0 {
                    println!("🎮 战斗开始! 初始波次: {}", status.current_wave);
                    self.last_wave_change_time = Instant::now();
                    break;
                }
            }
            thread::sleep(Duration::from_millis(1000));
        }

        self.execute_prep_logic(loadout);
        self.setup_view();

        println!("🤖 自动化监控中...");
        loop {
            // 战斗阶段：需要 TAB，用新正则
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