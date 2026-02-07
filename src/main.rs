// src/main.rs
use nzm_cmd::hardware::InputDevice;
use nzm_cmd::human::HumanDriver;
use nzm_cmd::nav::{NavEngine, NavResult};
use nzm_cmd::tower_defense::TowerDefenseApp;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use clap::Parser;
use screenshots::Screen; 

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// 硬件串口名称 (例如: COM9, /dev/ttyUSB0)
    #[arg(short, long, default_value = "COM3")]
    port: String,

    /// 导航目标界面名称 (例如: "空间站普通", "空间站炼狱")
    /// 这个名字将直接用于寻找对应的地图和策略文件
    #[arg(short, long, default_value = "空间站普通")]
    target: String,

    /// 运行测试模式 (可选: input, screen, ocr)
    #[arg(long)]
    test: Option<String>,
}

fn main() {
    let args = Args::parse();

    println!("========================================");
    println!("🚀 NZM_CMD 智能控制中心");
    println!("📍 端口: {}", args.port);
    if let Some(t) = &args.test {
        println!("🔧 模式: 测试 ({})", t);
    } else {
        println!("🎯 目标: {}", args.target);
    }
    println!("========================================");

    // 1. 硬件驱动初始化
    let (sw, sh) = (1920, 1080);
    let driver_arc = match InputDevice::new(&args.port, 115200, sw, sh) {
        Ok(d) => Arc::new(Mutex::new(d)),
        Err(e) => {
            println!("⚠️ 警告: 无法连接硬件 ({})", e);
            println!("⚠️ 进入无硬件模拟模式");
            unsafe { std::mem::transmute(Arc::new(Mutex::new(()))) } 
        }
    };

    // 启动心跳
    let hb = Arc::clone(&driver_arc);
    thread::spawn(move || loop {
        if let Ok(mut d) = hb.lock() { d.heartbeat(); }
        thread::sleep(Duration::from_secs(1));
    });

    // 2. 初始化驱动与引擎
    let human_driver = Arc::new(Mutex::new(
        HumanDriver::new(Arc::clone(&driver_arc), sw/2, sh/2)
    ));

    let engine = Arc::new(NavEngine::new("ui_map.toml", Arc::clone(&human_driver)));

    // ==========================================
    // 🔍 分发测试逻辑
    // ==========================================
    if let Some(mode) = args.test.as_deref() {
        // ... (测试代码保持不变) ...
        println!("⏳ 5秒后开始执行 [{}] 测试...", mode);
        thread::sleep(Duration::from_secs(5));
        match mode {
            "input" => run_input_test(human_driver),
            "screen" => run_screen_test(),
            "ocr" => run_ocr_test(engine),
            _ => println!("❌ 未知测试模式"),
        }
        return; 
    }

    // ==========================================
    // 🚀 正常业务流程
    // ==========================================
    println!("✅ 引擎就绪，5秒后开始自动导航...");
    thread::sleep(Duration::from_secs(5));

    println!("\n🔄 [主控] 正在导航至: {}...", args.target);
    let nav_result = engine.navigate(&args.target);

    match nav_result {
        NavResult::Handover(scene_id) => {
            println!("⚔️ [主控] 控制权移交: [{}] -> 启动塔防逻辑", scene_id);
            let mut td_app = TowerDefenseApp::new(Arc::clone(&human_driver), Arc::clone(&engine));
            
            // ✨✨✨ 核心修改：动态生成文件名 ✨✨✨
            // 规则：
            // 1. 地图文件 = "{目标名}地图.json"
            // 2. 策略文件 = "{目标名}策略.json"
            // 3. 陷阱配置 = "traps_config.json" (默认通用)
            let map_file = format!("{}地图.json", scene_id);
            let strategy_file = format!("{}策略.json", scene_id);
            let traps_file = "traps_config.json";

            println!("📂 自动加载配置:");
            println!("   📄 地图: {}", map_file);
            println!("   📄 策略: {}", strategy_file);
            println!("   📄 陷阱: {}", traps_file);

            // 调用 run，传入生成的路径
            td_app.run(&map_file, &strategy_file, traps_file);
        }
        NavResult::Success => println!("✅ [主控] 到达目标，任务完成。"),
        NavResult::Failed => println!("❌ [主控] 导航失败。"),
    }
}

// ----------------------------------------------------------------
// 🛠️ 测试函数实现
// ----------------------------------------------------------------

fn run_input_test(driver: Arc<Mutex<HumanDriver>>) {
    println!("Testing Mouse & Keyboard...");
    if let Ok(mut d) = driver.lock() {
        // 1. 鼠标方形移动测试
        println!("-> 移动鼠标 (矩形轨迹)");
        let start_x = 500;
        let start_y = 500;
        d.move_to_humanly(start_x, start_y, 0.5);
        d.move_to_humanly(start_x + 300, start_y, 0.5);
        d.move_to_humanly(start_x + 300, start_y + 300, 0.5);
        d.move_to_humanly(start_x, start_y + 300, 0.5);
        d.move_to_humanly(start_x, start_y, 0.5);

        // 2. 点击测试
        println!("-> 执行点击 (Click)");
        d.click_humanly(true, false, 0);
        thread::sleep(Duration::from_millis(500));

        // 3. 键盘输入测试
        println!("-> 模拟键盘输入 'hello 123'");
        d.type_humanly("hello 123", 60.0);
    }
    println!("Done.");
}

fn run_screen_test() {
    println!("Testing Screen Capture...");
    let start = Instant::now();
    let screens = Screen::all().unwrap_or_default();
    
    if let Some(screen) = screens.first() {
        println!("-> 检测到屏幕: {}x{}", screen.display_info.width, screen.display_info.height);
        match screen.capture() {
            Ok(image) => {
                let path = "debug_screenshot.png";
                image.save(path).unwrap();
                println!("✅ 截图成功! 已保存至: {} (耗时 {}ms)", path, start.elapsed().as_millis());
                println!("   请打开图片确认颜色和内容是否正常。");
            },
            Err(e) => println!("❌ 截图失败: {}", e),
        }
    } else {
        println!("❌ 未检测到显示器");
    }
}

fn run_ocr_test(engine: Arc<NavEngine>) {
    println!("Testing OCR Function...");
    // 定义一个测试区域 (例如屏幕左上角的一块区域，通常包含HUD信息)
    // 这里取 x=100, y=100, w=400, h=100
    let rect = [100, 100, 500, 200]; 
    
    println!("-> 正在识别区域: {:?}", rect);
    let start = Instant::now();
    let text = engine.ocr_area(rect);
    
    println!("----------------------------------------");
    println!("⏱️ 耗时: {} ms", start.elapsed().as_millis());
    println!("📝 识别结果: [{}]", text);
    println!("----------------------------------------");
    
    if text.is_empty() {
        println!("⚠️ 警告: 识别结果为空，请确认该区域有文字。");
    }
}