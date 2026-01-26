#![windows_subsystem = "windows"]

use eframe::egui::{self, Color32, Pos2, Rect, RichText, Sense, Stroke, Vec2};
use screenshots::Screen;
use std::fs;

// ==========================================
// 1. 数据结构 (与你的导航引擎完全匹配)
// ==========================================
#[derive(Clone, PartialEq)]
enum ElementKind {
    Anchor,
    Button { target: String },
}

#[derive(Clone)]
struct UIElementDraft {
    rect: Rect,        // 界面上的像素矩形
    ocr_text: String,  // 识别到的文字
    kind: ElementKind,
}

// ==========================================
// 2. 编辑器状态
// ==========================================
struct MapBuilderTool {
    texture: Option<egui::TextureHandle>,
    img_size: Vec2,         // 原始图片的尺寸
    scene_id: String,
    scene_name: String,
    
    // 交互
    start_pos: Option<Pos2>,
    current_rect: Option<Rect>,
    
    // 数据
    drafts: Vec<UIElementDraft>,
    toml_output: String,
}

impl MapBuilderTool {
    fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        Self {
            texture: None,
            img_size: Vec2::ZERO,
            scene_id: "lobby".into(),
            scene_name: "游戏大厅".into(),
            start_pos: None,
            current_rect: None,
            drafts: Vec::new(),
            toml_output: String::new(),
        }
    }

    fn capture(&mut self, ctx: &egui::Context) {
        let screen = Screen::all().unwrap()[0];
        if let Ok(image) = screen.capture() {
            self.img_size = Vec2::new(image.width() as f32, image.height() as f32);
            let pixels = image.to_rgba8();
            let color_img = egui::ColorImage::from_rgba_unmultiplied(
                [image.width() as usize, image.height() as usize], 
                pixels.as_flat_samples().as_slice()
            );
            self.texture = Some(ctx.load_texture("shot", color_img, Default::default()));
        }
    }

    // 🔥 建议：在这里调用你的 OCR 模块
    fn do_ocr(&self, _rect: Rect) -> String {
        // 实际开发中：
        // 1. 根据 _rect 从原始图片 buffer 中 crop 出一块
        // 2. 传给 PaddleOCR (ONNX) 识别
        // 3. 返回识别出的字符串
        "识别到的中文".to_string() 
    }

    fn build_toml(&mut self) {
        let mut toml = format!("# 场景定义：{}\n[[scenes]]\nid = \"{}\"\nname = \"{}\"\n", 
                                self.scene_name, self.scene_id, self.scene_name);
        
        // 生成锚点
        toml.push_str("anchors = [\n");
        for d in self.drafts.iter().filter(|d| matches!(d.kind, ElementKind::Anchor)) {
            toml.push_str(&format!("    {{ rect = [{}, {}, {}, {}], text = \"{}\" }},\n",
                d.rect.min.x as i32, d.rect.min.y as i32, d.rect.max.x as i32, d.rect.max.y as i32, d.ocr_text));
        }
        toml.push_str("]\n\n");

        // 生成跳转关系
        for d in self.drafts.iter().filter(|d| matches!(d.kind, ElementKind::Button{..})) {
            if let ElementKind::Button { target } = &d.kind {
                toml.push_str("[[scenes.transitions]]\n");
                toml.push_str(&format!("target = \"{}\"\n", target));
                toml.push_str(&format!("trigger_btn = [{}, {}]\n", d.rect.center().x as i32, d.rect.center().y as i32));
                toml.push_str("action = \"Click\"\n\n");
            }
        }
        self.toml_output = toml;
    }
}

// ==========================================
// 3. GUI 交互逻辑
// ==========================================
impl eframe::App for MapBuilderTool {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // 左侧面板：控制与数据展示
        egui::SidePanel::left("side").min_width(320.0).show(ctx, |ui| {
            ui.heading("🎯 MINKE UI 建模工具");
            ui.add_space(10.0);
            
            if ui.button("📸 截取屏幕").clicked() { self.capture(ctx); }
            
            ui.separator();
            ui.horizontal(|ui| { ui.label("场景ID:"); ui.text_edit_singleline(&mut self.scene_id); });
            ui.horizontal(|ui| { ui.label("名称:"); ui.text_edit_singleline(&mut self.scene_name); });

            ui.separator();
            if let Some(rect) = self.current_rect {
                ui.group(|ui| {
                    ui.label(RichText::new("已选中元素").color(Color32::YELLOW));
                    ui.label(format!("坐标: [{}, {}, {}, {}]", rect.min.x as i32, rect.min.y as i32, rect.max.x as i32, rect.max.y as i32));
                    
                    if ui.button("⚓ 添加为锚点 (用于定位)").clicked() {
                        let text = self.do_ocr(rect);
                        self.drafts.push(UIElementDraft { rect, ocr_text: text, kind: ElementKind::Anchor });
                        self.current_rect = None;
                    }
                    if ui.button("🔄 添加为跳转 (点击切换)").clicked() {
                        let text = self.do_ocr(rect);
                        self.drafts.push(UIElementDraft { rect, ocr_text: text, kind: ElementKind::Button { target: "next_scene".into() } });
                        self.current_rect = None;
                    }
                });
            }

            ui.separator();
            ui.label("当前场景元素列表:");
            egui::ScrollArea::vertical().max_height(200.0).show(ui, |ui| {
                let mut del = None;
                for (i, d) in self.drafts.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        let icon = if matches!(d.kind, ElementKind::Anchor) { "⚓" } else { "🖱️" };
                        ui.label(format!("{} {}", icon, d.ocr_text));
                        if let ElementKind::Button { target } = &mut d.kind {
                            ui.text_edit_singleline(target);
                        }
                        if ui.button("❌").clicked() { del = Some(i); }
                    });
                }
                if let Some(i) = del { self.drafts.remove(i); }
            });

            ui.separator();
            if ui.button("💾 生成 TOML 块").clicked() { self.build_toml(); }
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.text_edit_multiline(&mut self.toml_output);
            });
        });

        // 中央面板：可视化操作区
        egui::CentralPanel::default().show(ctx, |ui| {
            // 获取画布的实际显示区域
            let (resp, painter) = ui.allocate_painter(ui.available_size(), Sense::drag());
            
            if let Some(tex) = &self.texture {
                // 计算底图在画布中的显示位置（保持原始比例）
                let painter_size = resp.rect.size();
                let scale = (painter_size.x / self.img_size.x).min(painter_size.y / self.img_size.y);
                let draw_size = self.img_size * scale;
                let draw_rect = Rect::from_min_size(resp.rect.min, draw_size);

                painter.image(tex.id(), draw_rect, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);

                // --- 交互转换逻辑 ---
                let to_screen = |p: Pos2| draw_rect.min + (p.to_vec2() * scale);
                let from_screen = |p: Pos2| (p - draw_rect.min) / scale;

                // 绘制已保存元素
                for d in &self.drafts {
                    let color = if matches!(d.kind, ElementKind::Anchor) { Color32::GREEN } else { Color32::BLUE };
                    let screen_rect = Rect::from_min_max(to_screen(d.rect.min), to_screen(d.rect.max));
                    painter.rect_stroke(screen_rect, 2.0, Stroke::new(2.0, color));
                }

                // 处理拖拽
                if resp.drag_started() { self.start_pos = resp.interact_pointer_pos().map(from_screen); }
                if let (Some(start), Some(curr_raw)) = (self.start_pos, resp.interact_pointer_pos()) {
                    let curr = from_screen(curr_raw);
                    let rect = Rect::from_two_pos(start, curr);
                    
                    // 绘制正在拖拽的框
                    let preview_rect = Rect::from_min_max(to_screen(rect.min), to_screen(rect.max));
                    painter.rect_stroke(preview_rect, 0.0, Stroke::new(1.5, Color32::RED));

                    if resp.drag_released() {
                        self.current_rect = Some(rect);
                        self.start_pos = None;
                    }
                }
            } else {
                ui.centered_and_justified(|ui| ui.label("点击左侧『截取屏幕』开始工作"));
            }
        });
    }
}

fn main() -> eframe::Result<()> {
    let opts = eframe::NativeOptions { viewport: egui::ViewportBuilder::default().with_inner_size([1280.0, 800.0]), ..Default::default() };
    eframe::run_native("MINKE UI Mapper", opts, Box::new(|cc| {
        // 加载中文字体，确保侧边栏显示正常
        let mut fonts = egui::FontDefinitions::default();
        if let Ok(data) = fs::read("C:\\Windows\\Fonts\\msyh.ttc") { // 微软雅黑
            fonts.font_data.insert("my_font".to_owned(), egui::FontData::from_owned(data));
            fonts.families.get_mut(&egui::FontFamily::Proportional).unwrap().insert(0, "my_font".to_owned());
        }
        cc.egui_ctx.set_fonts(fonts);
        Box::new(MapBuilderTool::new(cc))
    }))
}