// src/main.rs
use minke_driver::InputDevice;
use minke_driver::human::HumanDriver;
use minke_driver::nav::NavEngine;
use minke_driver::tower_defense::TowerDefenseApp;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

fn main() {
    println!("========================================");
    println!("🛠️ MINKE 塔防模式 - 纯代码控制版");
    println!("========================================");

    let port_name = "COM9"; 
    let (sw, sh) = (1920, 1080);
    
    let driver_arc = match InputDevice::new(port_name, 115200, sw, sh) {
        Ok(d) => Arc::new(Mutex::new(d)),
        Err(e) => {
            // panic!("❌ 错误: 硬件未连接 ({})", e); // 正常调试用这行
            unsafe { std::mem::transmute(Arc::new(Mutex::new(()))) } // 无硬件调试用这行
        }
    };

    let hb = Arc::clone(&driver_arc);
    thread::spawn(move || loop {
        if let Ok(mut d) = hb.lock() { d.heartbeat(); }
        thread::sleep(Duration::from_secs(1));
    });

    let human_driver = Arc::new(Mutex::new(
        HumanDriver::new(Arc::clone(&driver_arc), sw/2, sh/2)
    ));

    let engine = Arc::new(NavEngine::new("ui_map.toml", Arc::clone(&human_driver)));
    println!("✅ 引擎初始化完成");

    println!("👉 请在 5 秒内切换到游戏窗口...");
    thread::sleep(Duration::from_secs(5));

    println!("\n🚀 [DEBUG] 启动逻辑...");

    let mut td_app = TowerDefenseApp::new(
        Arc::clone(&human_driver),
        Arc::clone(&engine) 
    );
    
    // 定义你要携带的塔 (名字必须和 traps_config.json 里的一致)
    let my_loadout = vec![
        "破坏者", 
        "自修复磁暴塔", 
        "防空导弹",
        "修理站"
    ];

    td_app.run(
        "空间站.json", 
        "strategy_01.json", 
        "traps_config.json", // 依然保留坐标配置，方便改 UI
        &my_loadout          // 传入要携带的塔列表
    );

    println!("✅ 执行完毕");
}