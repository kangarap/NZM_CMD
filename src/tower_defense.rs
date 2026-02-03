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

#[derive(Debug, Clone)]
pub struct TDConfig {
    pub hud_check_rect: [i32; 4],     // 初始识别区域
    pub hud_wave_loop_rect: [i32; 4], // 循环监控区域
    pub safe_zone: [i32; 4],
    pub screen_width: f32,
    pub screen_height: f32,
}

impl Default for TDConfig {
    fn default() -> Self {
        Self {
            hud_check_rect: [262, 16, 389, 97],
            hud_wave_loop_rect: [352, 279, 503, 360], 
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
    camera_offset_x: f32,
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
            camera_offset_x: 0.0,
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
                println!("❌ 策略 JSON 解析失败，请检查字段是否匹配");
            }
        }
    }

    // 🔥 核心修改：增加 use_tab 参数
    pub fn recognize_wave_status(&self, rect: [i32; 4], use_tab: bool) -> Option<WaveStatus> {
        const KEY_TAB: u8 = 0x2B; 

        // 1. 如果需要 TAB，先按住
        if use_tab {
            if let Ok(driver) = self.driver.lock() {
                if let Ok(mut dev) = driver.device.lock() {
                    dev.key_down(KEY_TAB, 0);
                }
            }
            // 等待 UI 弹出
            thread::sleep(Duration::from_millis(200));
        }

        // 2. OCR 识别
        let text: String = self.nav.ocr_area(rect);

        // 3. 如果按下了 TAB，现在处理松开和恢复逻辑
        if use_tab {
            // 松开
            if let Ok(driver) = self.driver.lock() {
                if let Ok(mut dev) = driver.device.lock() {
                    dev.key_up();
                }
            }

            // 再次点按以恢复状态 (Trigger Toggle)
            thread::sleep(Duration::from_millis(50));
            if let Ok(driver) = self.driver.lock() {
                if let Ok(mut dev) = driver.device.lock() {
                    dev.key_down(KEY_TAB, 0);
                }
            }
            thread::sleep(Duration::from_millis(50));
            if let Ok(driver) = self.driver.lock() {
                if let Ok(mut dev) = driver.device.lock() {
                    dev.key_up();
                }
            }
        }

        if text.is_empty() { return None; }

        let re_wave = Regex::new(r"波次(\d+)").unwrap();
        if let Some(caps) = re_wave.captures(&text) {
            let val = caps.get(1)?.as_str().parse::<i32>().ok()?;
            Some(WaveStatus { current_wave: val })
        } else { None }
    }

    fn validate_wave_transition(&mut self, detected_wave: i32) -> bool {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_wave_change_time).as_secs();
        let is_next_wave = detected_wave == self.last_confirmed_wave + 1;
        let is_long_enough = elapsed >= 60 || self.last_confirmed_wave == 0;

        if is_next_wave && is_long_enough {
            println!("✅ [Monitor] 确认进入新波次: {} -> {}", self.last_confirmed_wave, detected_wave);
            self.last_confirmed_wave = detected_wave;
            self.last_wave_change_time = now;
            true
        } else { false }
    }

    pub fn execute_wave_phase(&mut self, wave: i32, is_late: bool) {
        let phase_name = if is_late { "后期" } else { "前期" };
        println!("🚀 开始执行第 {} 波 [{}] 布防任务...", wave, phase_name);

        // 1. 拆除
        let to_demolish: Vec<DemolishEvent> = self.strategy_demolishes.iter()
            .filter(|d| d.wave_num == wave && d.is_late == is_late && !self.completed_demolish_uids.contains(&d.uid))
            .cloned().collect();
        if !to_demolish.is_empty() {
            println!("🔥 执行拆除任务: {} 个", to_demolish.len());
            self.execute_specific_demolishes(to_demolish);
        }

        // 2. 建造
        let to_place: Vec<BuildingExport> = self.strategy_buildings.iter()
            .filter(|b| b.wave_num == wave && b.is_late == is_late && !self.placed_uids.contains(&b.uid))
            .cloned().collect();
        if !to_place.is_empty() {
            self.execute_specific_placements(to_place);
        }

        // 3. 升级
        let to_upgrade: Vec<UpgradeEvent> = self.strategy_upgrades.iter()
            .filter(|u| u.wave_num == wave && u.is_late == is_late)
            .filter(|u| {
                let key = format!("{}-{}-{}", u.building_name, u.wave_num, u.is_late);
                !self.completed_upgrade_keys.contains(&key)
            })
            .cloned().collect();
        if !to_upgrade.is_empty() {
            self.execute_specific_upgrades(to_upgrade);
        }
    }

    fn execute_specific_demolishes(&mut self, tasks: Vec<DemolishEvent>) {
        let [sz_x1, sz_y1, sz_x2, sz_y2] = self.config.safe_zone;
        for d in tasks {
            if let Some((map_px, map_py)) = self.get_absolute_map_pixel(d.grid_x, d.grid_y, d.width, d.height) {
                self.ensure_target_in_safe_zone(map_px, map_py);
                let screen_x = map_px - self.camera_offset_x;
                let screen_y = map_py - self.camera_offset_y;
                let final_x = screen_x.clamp(sz_x1 as f32, sz_x2 as f32);
                let final_y = screen_y.clamp(sz_y1 as f32, sz_y2 as f32);

                if let Ok(mut driver) = self.driver.lock() {
                    driver.move_to_humanly(final_x as u16, final_y as u16, 0.4);
                    driver.click_humanly(true, false, 0); 
                    thread::sleep(Duration::from_millis(150));
                    driver.key_click('p'); 
                }
                self.completed_demolish_uids.insert(d.uid);
                println!("   -> 塔 (UID: {}) 已拆除", d.uid);
                thread::sleep(Duration::from_millis(300));
            }
        }
    }

    fn execute_specific_placements(&mut self, tasks: Vec<BuildingExport>) {
        let mut last_key: Option<char> = None;
        let [sz_x1, sz_y1, sz_x2, sz_y2] = self.config.safe_zone;
        for b in tasks {
            if let Some((map_px, map_py)) = self.get_absolute_map_pixel(b.grid_x, b.grid_y, b.width, b.height) {
                self.ensure_target_in_safe_zone(map_px, map_py);
                let screen_x = map_px - self.camera_offset_x;
                let screen_y = map_py - self.camera_offset_y;
                let final_x = screen_x.clamp(sz_x1 as f32, sz_x2 as f32);
                let final_y = screen_y.clamp(sz_y1 as f32, sz_y2 as f32);

                let key = self.get_trap_key(&b.name);
                if let Ok(mut d) = self.driver.lock() {
                    d.move_to_humanly(final_x as u16, final_y as u16, 0.35);
                    if Some(key) != last_key {
                        d.key_click(key);
                        last_key = Some(key);
                        thread::sleep(Duration::from_millis(200));
                    }
                    d.double_click_humanly(true, false);
                }
                self.placed_uids.insert(b.uid);
                thread::sleep(Duration::from_millis(250));
            }
        }
    }

    fn execute_specific_upgrades(&mut self, tasks: Vec<UpgradeEvent>) {
        for u in tasks {
            let key = self.get_trap_key(&u.building_name);
            if let Ok(mut d) = self.driver.lock() {
                println!("   -> 长按 '{}' (800ms) 以升级: {}", key, u.building_name);
                d.key_hold(key, 800); 
            }
            let key_str = format!("{}-{}-{}", u.building_name, u.wave_num, u.is_late);
            self.completed_upgrade_keys.insert(key_str);
            thread::sleep(Duration::from_millis(400));
        }
    }

    fn ensure_target_in_safe_zone(&mut self, _tx: f32, ty: f32) {
        let meta = match &self.map_meta { Some(m) => m, None => return };
        let [_, z_y1, _, z_y2] = self.config.safe_zone;
        let max_offset_y = (meta.bottom - self.config.screen_height).max(0.0);
        let is_bottom_zone = ty > (meta.bottom - (self.config.screen_height - z_y1 as f32));

        loop {
            let rel_y = ty - self.camera_offset_y;
            if rel_y >= z_y1 as f32 && rel_y <= z_y2 as f32 { break; }
            
            let target_offset = if is_bottom_zone {
                max_offset_y
            } else {
                let safe_center_y = (z_y1 + z_y2) as f32 / 2.0;
                (self.camera_offset_y + (rel_y - safe_center_y)).clamp(0.0, max_offset_y)
            };
            
            let dist = target_offset - self.camera_offset_y;
            if dist.abs() < 5.0 { break; }

            if let Ok(mut human) = self.driver.lock() {
                let key = if dist > 0.0 { 's' } else { 'w' };
                human.key_hold(key, (dist.abs() / self.move_speed * 1000.0) as u64);
                self.camera_offset_y = target_offset;
            }
            thread::sleep(Duration::from_millis(400));
            if is_bottom_zone { break; }
        }
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
                for item in items { self.trap_lookup.insert(item.name.clone(), item); }
            }
        }
    }

    pub fn setup_view(&mut self) {
        println!("🔭 对齐左上角边界...");
        if let Ok(mut human) = self.driver.lock() {
            human.key_click('o');
            thread::sleep(Duration::from_secs(2));
            for _ in 1..=7 {
                for _ in 0..12 { human.mouse_scroll(-120); thread::sleep(Duration::from_millis(30)); }
                thread::sleep(Duration::from_millis(300));
            }
            for _ in 1..=4 {
                human.key_hold('w', 500); thread::sleep(Duration::from_millis(50));
                human.key_hold('a', 500); thread::sleep(Duration::from_millis(50));
            }
            human.key_hold('w', 800); human.key_hold('a', 800);
        }
        self.camera_offset_x = 0.0;
        self.camera_offset_y = 0.0;
    }

    pub fn execute_prep_logic(&self, loadout: &[&str]) {
        println!("🔧 执行赛前准备...");
        if let Ok(mut human) = self.driver.lock() {
            human.key_click('n'); thread::sleep(Duration::from_millis(1000));
            human.move_to_humanly(212, 294, 0.5); human.click_humanly(true, false, 0);
        }
        self.select_loadout(loadout);
        if let Ok(mut human) = self.driver.lock() {
            human.key_click('n'); thread::sleep(Duration::from_millis(500));
        }
    }

    pub fn select_loadout(&self, tower_names: &[&str]) {
        for name in tower_names.iter().take(4) {
            if let Some(config) = self.trap_lookup.get(*name) {
                let [x, y] = config.select_pos;
                if let Ok(mut d) = self.driver.lock() {
                    d.move_to_humanly(x as u16, y as u16, 0.5); d.click_humanly(true, false, 0);
                }
                thread::sleep(Duration::from_millis(400));
            }
        }
    }

    fn get_absolute_map_pixel(&self, gx: usize, gy: usize, w: usize, h: usize) -> Option<(f32, f32)> {
        let meta = self.map_meta.as_ref()?;
        let sx = meta.offset_x + ((gx as f32 + w as f32 / 2.0) * meta.grid_pixel_size);
        let sy = meta.offset_y + ((gy as f32 + h as f32 / 2.0) * meta.grid_pixel_size);
        Some((sx, sy))
    }

    fn get_trap_key(&self, name: &str) -> char {
        let index = self.active_loadout.iter().position(|t| t == name).unwrap_or(0);
        match index { 0 => '4', 1 => '5', 2 => '6', 3 => '7', _ => '1' }
    }

    pub fn run(&mut self, terrain_p: &str, strategy_p: &str, trap_p: &str, loadout: &[&str]) {
        self.active_loadout = loadout.iter().map(|&s| s.to_string()).collect();
        self.load_map_terrain(terrain_p);
        self.load_strategy(strategy_p);
        self.load_trap_config(trap_p);

        if let Ok(mut human) = self.driver.lock() {
            println!("👆 点击游戏入口...");
            human.move_to_humanly(1700, 950, 0.5); human.click_humanly(true, false, 0);
            human.move_to_humanly(1110, 670, 0.5); human.click_humanly(true, false, 0);
        }

        println!("⏳ 等待战斗开始...");
        loop {
            // 🔥 初始阶段：不需要 TAB
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
            // 🔥 战斗阶段：需要 TAB
            if let Some(status) = self.recognize_wave_status(self.config.hud_wave_loop_rect, true) {
                if self.validate_wave_transition(status.current_wave) {
                    let current_wave = status.current_wave;
                    self.execute_wave_phase(current_wave, false);
                    println!("🔔 波次 {} 前期完成，按 G 开战", current_wave);
                    if let Ok(mut d) = self.driver.lock() { d.key_click('g'); }
                    thread::sleep(Duration::from_secs(1));
                    self.execute_wave_phase(current_wave, true);
                }
            }
            thread::sleep(Duration::from_millis(10000));
        }
    }
}