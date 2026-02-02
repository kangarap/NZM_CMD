// src/tower_defense.rs
use crate::human::HumanDriver;
use crate::nav::NavEngine;
use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

// ==========================================
// 1. 数据结构协议
// ==========================================

#[derive(Debug, Clone)]
pub struct TDConfig {
    pub hud_check_rect: [i32; 4],
    pub safe_zone: [i32; 4], 
    pub screen_width: f32,
    pub screen_height: f32,
}

impl Default for TDConfig {
    fn default() -> Self {
        Self {
            hud_check_rect: [845, 88, 1098, 175],
            // 严格安全区：确保点击不会触发任务栏或顶层 UI
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
    pub bottom: f32, // 地图最下端绝对 Y 像素坐标
}

#[derive(Deserialize, Debug, Clone)]
pub struct LayerData {
    pub major_z: i32,
    pub elevation_grid: Vec<Vec<i8>>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct BuildingExport {
    pub uid: usize,
    pub name: String,
    pub grid_x: usize,
    pub grid_y: usize,
    pub width: usize,
    pub height: usize,
}

#[derive(Deserialize, Debug, Clone)]
pub struct MapTerrainExport {
    pub map_name: String,
    pub meta: MapMeta,
    pub layers: Vec<LayerData>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct MapBuildingsExport {
    pub map_name: String,
    pub buildings: Vec<BuildingExport>,
}

// ==========================================
// 2. 塔防模块实现
// ==========================================
pub struct TowerDefenseApp {
    driver: Arc<Mutex<HumanDriver>>,
    nav: Arc<NavEngine>,
    config: TDConfig,
    map_meta: Option<MapMeta>,
    strategy: Vec<BuildingExport>,
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
            strategy: Vec::new(),
            trap_lookup: HashMap::new(),
            active_loadout: Vec::new(),
            camera_offset_x: 0.0,
            camera_offset_y: 0.0,
            move_speed: 720.0, 
        }
    }

    // --- 数据加载 ---

    pub fn load_map_terrain(&mut self, path: &str) {
        if let Ok(c) = fs::read_to_string(path) {
            if let Ok(data) = serde_json::from_str::<MapTerrainExport>(&c) { 
                println!("📊 加载地图: {}, 底部极限: {:.1}", data.map_name, data.meta.bottom);
                self.map_meta = Some(data.meta); 
            }
        }
    }

    pub fn load_strategy(&mut self, path: &str) {
        if let Ok(c) = fs::read_to_string(path) {
            if let Ok(data) = serde_json::from_str::<MapBuildingsExport>(&c) { 
                self.strategy = data.buildings; 
                println!("🏗️ 加载策略: {} 个建筑", self.strategy.len());
            }
        }
    }

    pub fn load_trap_config(&mut self, json_path: &str) {
        if let Ok(c) = fs::read_to_string(json_path) {
            if let Ok(items) = serde_json::from_str::<Vec<TrapConfigItem>>(&c) {
                for item in items { self.trap_lookup.insert(item.name.clone(), item); }
                println!("🎒 加载 {} 个陷阱 UI 坐标", self.trap_lookup.len());
            }
        }
    }

    // --- 核心控制流 ---

    /// 智能视口追踪：支持常规居中和底边撞击模式
    fn ensure_target_in_safe_zone(&mut self, _tx: f32, ty: f32) {
        let meta = match &self.map_meta { Some(m) => m, None => return };
        let [_, z_y1, _, z_y2] = self.config.safe_zone;
        
        // 物理滚动上限
        let max_offset_y = (meta.bottom - self.config.screen_height).max(0.0);
        // 如果目标点在地图底边往上一个视口内，判定为进入“底边操作区”
        let is_bottom_zone = ty > (meta.bottom - (self.config.screen_height - z_y1 as f32));

        loop {
            let rel_y = ty - self.camera_offset_y;

            // 1. 如果已在安全区内，直接通过
            if rel_y >= z_y1 as f32 && rel_y <= z_y2 as f32 {
                break; 
            }

            // 2. 决定目标偏移量
            let target_offset = if is_bottom_zone {
                println!("📍 目标处于底部区域，直接撞底对齐");
                max_offset_y
            } else {
                let safe_center_y = (z_y1 + z_y2) as f32 / 2.0;
                let diff = rel_y - safe_center_y;
                (self.camera_offset_y + diff).clamp(0.0, max_offset_y)
            };

            let actual_move_dist = target_offset - self.camera_offset_y;

            // 3. 物理撞墙检查
            if actual_move_dist.abs() < 5.0 { break; }

            // 4. 执行按键
            if let Ok(mut human) = self.driver.lock() {
                let key = if actual_move_dist > 0.0 { 's' } else { 'w' };
                let duration = (actual_move_dist.abs() / self.move_speed * 1000.0) as u64;
                println!("🔄 [Camera] 修正视角: {} {}ms", key, duration);
                human.key_hold(key, duration);
                self.camera_offset_y = target_offset;
            }
            thread::sleep(Duration::from_millis(400)); 
            
            if is_bottom_zone { break; }
        }
    }

    pub fn execute_all_placements(&mut self) {
        println!("🏗️ 开始执行策略布阵...");
        let mut last_key: Option<char> = None;
        let tasks = self.strategy.clone();
        let [sz_x1, sz_y1, sz_x2, sz_y2] = self.config.safe_zone;

        for b in tasks {
            let (map_px, map_py) = match self.get_absolute_map_pixel(b.grid_x, b.grid_y, b.width, b.height) {
                Some(p) => p,
                None => continue,
            };

            // 自动调整摄像机
            self.ensure_target_in_safe_zone(map_px, map_py);

            // 计算相对于当前屏幕的坐标
            let screen_x = map_px - self.camera_offset_x;
            let screen_y = map_py - self.camera_offset_y;

            // 越界彻底拦截 (防止鼠标飞出 1920 范围)
            if screen_x < 0.0 || screen_x > self.config.screen_width || 
               screen_y < 0.0 || screen_y > self.config.screen_height {
                println!("❌ [跳过] {} 坐标非法: ({:.0},{:.0})", b.name, screen_x, screen_y);
                continue;
            }

            // 强制钳位在安全区内
            let final_x = screen_x.clamp(sz_x1 as f32, sz_x2 as f32);
            let final_y = screen_y.clamp(sz_y1 as f32, sz_y2 as f32);

            let key = self.get_trap_key(&b.name);
            println!("   -> 放置 [{}] (UID:{}) @ 屏幕({:.0},{:.0})", b.name, b.uid, final_x, final_y);

            if let Ok(mut d) = self.driver.lock() {
                d.move_to_humanly(final_x as u16, final_y as u16, 0.4);
                thread::sleep(Duration::from_millis(200));

                if Some(key) != last_key {
                    d.key_click(key);
                    last_key = Some(key);
                    thread::sleep(Duration::from_millis(300));
                }
                d.double_click_humanly(true, false);
            }
            thread::sleep(Duration::from_millis(300));
        }
        println!("✅ 放置任务完成");
    }

    // --- 准备动作 ---

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
            human.key_hold('w', 800);
            human.key_hold('a', 800);
        }
        self.camera_offset_x = 0.0;
        self.camera_offset_y = 0.0;
        thread::sleep(Duration::from_millis(500));
    }

    pub fn select_loadout(&self, tower_names: &[&str]) {
        println!("🎒 选择防御塔组合...");
        for (i, name) in tower_names.iter().take(4).enumerate() {
            if let Some(config) = self.trap_lookup.get(*name) {
                let [x, y] = config.select_pos;
                if x == 0 && y == 0 { continue; }
                if let Ok(mut d) = self.driver.lock() {
                    d.move_to_humanly(x as u16, y as u16, 0.5);
                    d.click_humanly(true, false, 0);
                }
                thread::sleep(Duration::from_millis(400));
            }
        }
    }

    pub fn execute_prep_logic(&self, loadout: &[&str]) {
        println!("🔧 执行赛前 W+Space 动作...");
        if let Ok(mut human) = self.driver.lock() {
            let w_code = 0x1A; let space_code = 0x2C;
            if let Ok(mut dev) = human.device.lock() { dev.key_down(w_code, 0); }
            for _ in 0..3 {
                thread::sleep(Duration::from_millis(600)); 
                if let Ok(mut dev) = human.device.lock() {
                    dev.key_down(space_code, 0);
                    thread::sleep(Duration::from_millis(50));
                    dev.key_up(); 
                    dev.key_down(w_code, 0);
                }
            }
            thread::sleep(Duration::from_millis(200)); 
            if let Ok(mut dev) = human.device.lock() { dev.key_up(); }
            
            thread::sleep(Duration::from_millis(800));
            human.key_click('n');
            thread::sleep(Duration::from_millis(1200));
            
            // 确认点选坐标
            human.move_to_humanly(212, 294, 0.5); 
            human.click_humanly(true, false, 0);
        }

        self.select_loadout(loadout);

        if let Ok(mut human) = self.driver.lock() {
            human.key_click('n');
            thread::sleep(Duration::from_millis(500));
        }
    }

    // --- 内部数学 ---

    fn get_absolute_map_pixel(&self, gx: usize, gy: usize, w: usize, h: usize) -> Option<(f32, f32)> {
        let meta = self.map_meta.as_ref()?;
        let center_gx = gx as f32 + (w as f32 / 2.0);
        let center_gy = gy as f32 + (h as f32 / 2.0);
        let sx = meta.offset_x + (center_gx * meta.grid_pixel_size);
        let sy = meta.offset_y + (center_gy * meta.grid_pixel_size);
        Some((sx, sy))
    }

    fn get_trap_key(&self, name: &str) -> char {
        let index = self.active_loadout.iter().position(|t| t == name).unwrap_or(0);
        match index { 
            0 => '4', 
            1 => '5', 
            2 => '6', 
            3 => '7', 
            _ => '1' 
        }
    }

    pub fn run(&mut self, terrain_p: &str, strategy_p: &str, trap_p: &str, loadout: &[&str]) { 
        self.active_loadout = loadout.iter().map(|&s| s.to_string()).collect();
        self.load_map_terrain(terrain_p);
        self.load_strategy(strategy_p);
        self.load_trap_config(trap_p);

        self.execute_prep_logic(loadout);
        self.setup_view();
        self.execute_all_placements();
    }
}