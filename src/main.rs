use serde::Deserialize;
use std::collections::{HashMap, VecDeque};
use std::fs;

// --- 数据结构定义 ---

#[derive(Deserialize, Debug, Clone)]
struct Anchor {
    rect: [u32; 4],
    text: String,
}

#[derive(Deserialize, Debug, Clone)]
struct Transition {
    target: String,
    trigger_btn: [u32; 2],
    action: String,
}

#[derive(Deserialize, Debug, Clone)]
struct Scene {
    id: String,
    name: String,
    anchors: Vec<Anchor>,
    transitions: Vec<Transition>,
}

#[derive(Deserialize, Debug)]
struct UIMap {
    scenes: Vec<Scene>,
}

// --- 导航引擎 ---

struct NavEngine {
    scenes: HashMap<String, Scene>,
}

impl NavEngine {
    fn new(config_path: &str) -> Self {
        let content = fs::read_to_string(config_path).expect("无法读取配置文件");
        let ui_map: UIMap = toml::from_str(&content).expect("TOML 格式错误");
        
        let mut scene_dict = HashMap::new();
        for s in ui_map.scenes {
            scene_dict.insert(s.id.clone(), s);
        }
        
        Self { scenes: scene_dict }
    }

    // 核心算法：BFS 查找从当前页面到目标页面的最短操作序列
    fn find_path(&self, start_id: &str, target_id: &str) -> Option<Vec<Transition>> {
        let mut queue = VecDeque::new();
        // 记录访问过的节点及到达它的转换动作
        let mut visited: HashMap<String, Option<(String, Transition)>> = HashMap::new();

        queue.push_back(start_id.to_string());
        visited.insert(start_id.to_string(), None);

        while let Some(current_id) = queue.pop_front() {
            if current_id == target_id {
                // 溯源找到完整路径
                let mut path = Vec::new();
                let mut curr = target_id.to_string();
                while let Some(Some((prev, trans))) = visited.get(&curr) {
                    path.push(trans.clone());
                    curr = prev.clone();
                }
                path.reverse();
                return Some(path);
            }

            if let Some(scene) = self.scenes.get(&current_id) {
                for trans in &scene.transitions {
                    if !visited.contains_key(&trans.target) {
                        visited.insert(trans.target.clone(), Some((current_id.clone(), trans.clone())));
                        queue.push_back(trans.target.clone());
                    }
                }
            }
        }
        None
    }

    // 模拟 OCR 识别当前场景
    fn identify_current_scene(&self) -> String {
        // 这里对接你的 OCR 逻辑，遍历所有场景的 anchors 进行匹配
        // 演示目的：直接返回大厅
        "lobby".to_string()
    }

    // 执行跳转指令
    fn jump_to(&self, target_id: &str) {
        let current_id = self.identify_current_scene();
        println!("当前位置: {}, 目标位置: {}", current_id, target_id);

        if let Some(path) = self.find_path(&current_id, target_id) {
            println!("找到路径，共 {} 步:", path.len());
            for step in path {
                println!("执行动作: {:?} -> 点击坐标 {:?}", step.action, step.trigger_btn);
                // 这里调用你的 ESP32 串口发送函数:
                // serial_send_click(step.trigger_btn[0], step.trigger_btn[1]);
                
                // 关键一步：每步执行完，都要 OCR 确认是否到达了下一阶段
                std::thread::sleep(std::time::Duration::from_millis(1500)); 
            }
            println!("跳转完成！");
        } else {
            println!("错误：无法从 {} 到达 {}", current_id, target_id);
        }
    }
}

fn main() {
    let engine = NavEngine::new("ui_map.toml");
    
    // 命令：从大厅跳转到地图选择页面
    engine.jump_to("map_select");
}