/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

#[path = "../doc/mod.rs"]
mod doc;
#[path = "../layout/mod.rs"]
mod layout;
#[path = "../util/mod.rs"]
mod util;
#[path = "../comp/mod.rs"]
mod comp;

use comp::{color, color::Color};

use font::FontIndex;
use font::prelude::*;
use gleam::gl;
use winit::dpi::PhysicalSize;
use winit::event::Event;
use winit::event::ModifiersState;
use winit::event::WindowEvent;
use winit::event_loop::ControlFlow;
use winit::window::CursorIcon;
use std::rc::Rc;
use std::time::Instant;
use webrender::api::*;
use webrender::api::units::*;
use webrender::render_api::*;
use webrender::FastHashMap;
use webrender::DebugFlags;
use winit::platform::run_return::EventLoopExtRunReturn;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

use crate::layout::Alignment;
use crate::layout::Direction;
use crate::layout::ExtendTo;
use crate::layout::LayoutContext;
use crate::layout::Paragraph;
use crate::layout::Selection;

struct Notifier {
    events_proxy: winit::event_loop::EventLoopProxy<()>,
}

impl Notifier {
    fn new(events_proxy: winit::event_loop::EventLoopProxy<()>) -> Notifier {
        Notifier { events_proxy }
    }
}

impl RenderNotifier for Notifier {
    fn clone(&self) -> Box<dyn RenderNotifier> {
        Box::new(Notifier {
            events_proxy: self.events_proxy.clone(),
        })
    }

    fn wake_up(&self, _composite_needed: bool) {
        #[cfg(not(target_os = "android"))]
        let _ = self.events_proxy.send_event(());
    }

    fn new_frame_ready(&self,
                       _: DocumentId,
                       _scrolled: bool,
                       composite_needed: bool,
                       _: FramePublishId) {
        self.wake_up(composite_needed);
    }
}

struct GlWindow {
    window: winit::window::Window,
    context: surfman::Context,
    device: surfman::Device,
    gl: Rc<dyn gl::Gl>,
    renderer: Option<webrender::Renderer>,
    name: &'static str,
    pipeline_id: PipelineId,
    document_id: DocumentId,
    epoch: Epoch,
    api: RenderApi,
    font_instance_key: FontInstanceKey,
}

impl Drop for GlWindow {
    fn drop(&mut self) {
        self.device.destroy_context(&mut self.context).unwrap();
        self.renderer.take().unwrap().deinit();
    }
}

impl GlWindow {
    fn new(event_loop: &winit::event_loop::EventLoop<()>, name: &'static str, clear_color: ColorF) -> Self {
        let window_builder = winit::window::WindowBuilder::new()
            .with_title(name)
	    .with_inner_size(winit::dpi::PhysicalSize::new(1024, 768));
        let window = window_builder.build(event_loop).unwrap();

        let connection = surfman::Connection::from_winit_window(&window).unwrap();
        let widget = connection.create_native_widget_from_winit_window(&window).unwrap();
        let adapter = connection.create_adapter().unwrap();
        let mut device = connection.create_device(&adapter).unwrap();
        let (major, minor) = match device.gl_api() {
            surfman::GLApi::GL => (3, 2),
            surfman::GLApi::GLES => (3, 0),
        };
        let context_descriptor = device.create_context_descriptor(&surfman::ContextAttributes {
            version: surfman::GLVersion {
                major,
                minor,
            },
            flags: surfman::ContextAttributeFlags::ALPHA |
            surfman::ContextAttributeFlags::DEPTH |
            surfman::ContextAttributeFlags::STENCIL,
        }).unwrap();
        let mut context = device.create_context(&context_descriptor, None).unwrap();
        device.make_context_current(&context).unwrap();

        let gl = match device.gl_api() {
            surfman::GLApi::GL => unsafe {
                gl::GlFns::load_with(
                    |symbol| device.get_proc_address(&context, symbol) as *const _
                )
            },
            surfman::GLApi::GLES => unsafe {
                gl::GlesFns::load_with(
                    |symbol| device.get_proc_address(&context, symbol) as *const _
                )
            },
        };
        let gl = gl::ErrorCheckingGl::wrap(gl);

        let surface = device.create_surface(
            &context,
            surfman::SurfaceAccess::GPUOnly,
            surfman::SurfaceType::Widget { native_widget: widget },
        ).unwrap();
        device.bind_surface_to_context(&mut context, surface).unwrap();

        let opts = webrender::WebRenderOptions {
            clear_color,
            ..webrender::WebRenderOptions::default()
        };

        let device_size = {
            let size = window
                .inner_size();

            DeviceIntSize::new(size.width as i32, size.height as i32)
        };
        let notifier = Box::new(Notifier::new(event_loop.create_proxy()));
        let (renderer, sender) = webrender::create_webrender_instance(gl.clone(), notifier, opts, None).unwrap();
        let mut api = sender.create_api();
        let document_id = api.add_document(device_size);

        let epoch = Epoch(0);
        let pipeline_id = PipelineId(0, 0);
        let mut txn = Transaction::new();

        let font_key = api.generate_font_key();
        let stretch = Stretch::NORMAL;
        let style = Style::Normal;
        let weight = Weight::NORMAL;
        if let Some(font) = FontIndex::global().query(
            "Droid Sans Mono",
            swash::Attributes::new(stretch, weight, style),
        ) {
            txn.add_native_font(font_key, NativeFontHandle(font.id().0));
            println!("font {:?} {:?}", font.family_name(), font.attributes());
        }

        let font_instance_key = api.generate_font_instance_key();
        txn.add_font_instance(font_instance_key, font_key, 32.0, None, None, Vec::new());

        api.send_transaction(document_id, txn);

        GlWindow {
            window,
            device,
            context,
            renderer: Some(renderer),
            name,
            epoch,
            pipeline_id,
            document_id,
            api,
            font_instance_key,
            gl,
        }
    }

    pub fn id(&self) -> winit::window::WindowId {
        self.window.id()
    }

    fn set_flags(&mut self) {
        println!("set flags {}", &self.name);
        self.api.send_debug_cmd(DebugCommand::SetFlags(DebugFlags::PROFILER_DBG));
    }

    fn resize(&mut self, size: PhysicalSize<u32>) {
        // todo!()
    }

    pub fn scale_factor(&self) -> f64 {
        self.window.scale_factor()
    }

    pub fn set_title(&self, title: &str) {
        self.window.set_title(title)
    }

    pub fn request_redraw(&self) {
        self.window.request_redraw()
    }

    pub fn inner_size(&self) -> PhysicalSize<u32> {
        self.window.inner_size()
    }

    pub fn set_cursor_icon(&self, cursor: CursorIcon) {
        self.window.set_cursor_icon(cursor);
    }

    //TODO: enable rust log
    fn redraw(&mut self) {
        let renderer = self.renderer.as_mut().unwrap();
        let api = &mut self.api;

        self.device.make_context_current(&self.context).unwrap();

        let device_pixel_ratio = self.window.scale_factor() as f32;
        let device_size = {
            let size = self
                .window
                .inner_size();
            DeviceIntSize::new(size.width as i32, size.height as i32)
        };
        let layout_size = device_size.to_f32() / euclid::Scale::new(device_pixel_ratio);
        let mut txn = Transaction::new();
        let mut builder = DisplayListBuilder::new(self.pipeline_id);
        let space_and_clip = SpaceAndClipInfo::root_scroll(self.pipeline_id);
        builder.begin();

        let bounds = LayoutRect::from_size(layout_size);
        builder.push_simple_stacking_context(
            bounds.min,
            space_and_clip.spatial_id,
            PrimitiveFlags::IS_BACKFACE_VISIBLE,
        );

        builder.push_rect(
            &CommonItemProperties::new(
                LayoutRect::from_origin_and_size(
                    LayoutPoint::new(100.0, 200.0),
                    LayoutSize::new(100.0, 200.0),
                ),
                space_and_clip,
            ),
            LayoutRect::from_origin_and_size(
                LayoutPoint::new(100.0, 200.0),
                LayoutSize::new(100.0, 200.0),
            ),
            ColorF::new(0.0, 1.0, 0.0, 1.0));

        let text_bounds = LayoutRect::from_origin_and_size(
            LayoutPoint::new(100.0, 50.0),
            LayoutSize::new(700.0, 200.0)
        );
        let glyphs = vec![
            GlyphInstance {
                index: 48,
                point: LayoutPoint::new(100.0, 100.0),
            },
            GlyphInstance {
                index: 68,
                point: LayoutPoint::new(150.0, 100.0),
            },
            GlyphInstance {
                index: 80,
                point: LayoutPoint::new(200.0, 100.0),
            },
            GlyphInstance {
                index: 82,
                point: LayoutPoint::new(250.0, 100.0),
            },
            GlyphInstance {
                index: 81,
                point: LayoutPoint::new(300.0, 100.0),
            },
            GlyphInstance {
                index: 3,
                point: LayoutPoint::new(350.0, 100.0),
            },
            GlyphInstance {
                index: 86,
                point: LayoutPoint::new(400.0, 100.0),
            },
            GlyphInstance {
                index: 79,
                point: LayoutPoint::new(450.0, 100.0),
            },
            GlyphInstance {
                index: 72,
                point: LayoutPoint::new(500.0, 100.0),
            },
            GlyphInstance {
                index: 83,
                point: LayoutPoint::new(550.0, 100.0),
            },
            GlyphInstance {
                index: 87,
                point: LayoutPoint::new(600.0, 100.0),
            },
            GlyphInstance {
                index: 17,
                point: LayoutPoint::new(650.0, 100.0),
            },
        ];

        builder.push_text(
            &CommonItemProperties::new(
                text_bounds,
                space_and_clip,
            ),
            text_bounds,
            &glyphs,
            self.font_instance_key,
            ColorF::new(1.0, 1.0, 0.0, 1.0),
            None,
        );

        builder.pop_stacking_context();

        txn.set_display_list(
            self.epoch,
            builder.end(),
        );
        txn.set_root_pipeline(self.pipeline_id);
        txn.generate_frame(0, RenderReasons::empty());
        api.send_transaction(self.document_id, txn);

        let framebuffer_object = self
            .device
            .context_surface_info(&self.context)
            .unwrap()
            .unwrap()
            .framebuffer_object;
        self.gl.bind_framebuffer(gl::FRAMEBUFFER, framebuffer_object);
        assert_eq!(self.gl.check_frame_buffer_status(gleam::gl::FRAMEBUFFER), gl::FRAMEBUFFER_COMPLETE);

        renderer.update();
        renderer.render(device_size, 0).unwrap();

        let mut surface = self.device.unbind_surface_from_context(&mut self.context).unwrap().unwrap();
        self.device.present_surface(&self.context, &mut surface).unwrap();
        self.device.bind_surface_to_context(&mut self.context, surface).unwrap();
    }
}

fn main() {
    // install global collector configured based on EMACSNG_LOG env var.
    #[cfg(debug_assertions)]
    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(EnvFilter::from_env("RUST_LOG"))
        .init();

    log::trace!("log");

    use clipboard2::Clipboard;
    let clipboard = clipboard2::SystemClipboard::new().unwrap();

    let mut event_loop = winit::event_loop::EventLoop::new();
    let mut windows = FastHashMap::default();

    let gl_window1 = GlWindow::new(&event_loop, "Swash demo", ColorF::new(0.3, 0.0, 0.0, 1.0));

    const MARGIN: f32 = 12.;
    let mut keymods = winit::event::ModifiersState::default();
    let mut dpi = gl_window1.scale_factor() as f32;
    let mut margin = MARGIN * dpi;
    let fonts = layout::FontLibrary::default();
    let mut layout_context = LayoutContext::new(&fonts);
    let initial_size = gl_window1.inner_size();
    let mut paragraph = Paragraph::new();
    let mut doc = build_document();
    // println!("{:?}", &doc);
    let mut first_run = true;
    let mut selection = Selection::default();
    let mut selection_rects: Vec<[f32; 4]> = Vec::new();
    let mut selecting = false;
    let mut selection_changed = false;
    let mut extend_to = ExtendTo::Point;
    let mut inserted = None;
    let mut last_time = Instant::now();
    let mut frame_count = 0;
    let mut total_time = 0f32;
    let mut title = String::from("");
    let mut mx = 0.;
    let mut my = 0.;
    let mut clicks = 0;
    let mut click_time = Instant::now();
    let mut cursor_on = true;
    let mut cursor_time = 0.;
    let mut needs_update = true;
    let mut size_changed = true;
    let mut dark_mode = false;
    let mut align = Alignment::Start;
    let mut always_update = false;

    // win1.set_cursor_icon(winit::window::CursorIcon::Text);

    let win1_id = gl_window1.id();
    windows.insert(win1_id, gl_window1);

    event_loop.run_return(|event, _elwt, control_flow| {
        *control_flow = winit::event_loop::ControlFlow::Wait;
    // event_loop.run(move |event, _, control_flow| {
    //     //println!("{:?}", event);
    //     *control_flow = ControlFlow::Poll;
        match event {
            Event::LoopDestroyed => return,
            Event::WindowEvent { event, window_id } => match event {
                WindowEvent::Resized(physical_size) => {
                    let window: &mut GlWindow = windows.get_mut(&window_id).unwrap();
                    window.resize(physical_size);
                    selection_changed = true;
                    size_changed = true;
                }
                WindowEvent::CloseRequested => *control_flow = ControlFlow::Exit,
                WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                    let window: &mut GlWindow = windows.get_mut(&window_id).unwrap();
                    dpi = scale_factor as f32;
                    margin = MARGIN * dpi;
                    needs_update = true;
                    selection_changed = true;
                    window.request_redraw();
                }
                WindowEvent::ModifiersChanged(mods) => keymods = mods,
                WindowEvent::CursorMoved { position, .. } => {
                    mx = position.x as f32;
                    my = position.y as f32;
                    if selecting {
                        selection =
                            selection.extend_to(&paragraph, mx - margin, my - margin, extend_to);
                        selection_changed = true;
                        cursor_time = 0.;
                        cursor_on = true;
                    }
                }
                WindowEvent::MouseInput { state, button, .. } => {
                    use winit::event::{ElementState, MouseButton};
                    if button != MouseButton::Left {
                        return;
                    }
                    cursor_time = 0.;
                    cursor_on = true;
                    if state == ElementState::Pressed {
                        let now = Instant::now();
                        if now.duration_since(click_time).as_secs_f32() < 0.25 {
                            if clicks == 3 {
                                clicks = 0;
                            }
                            clicks += 1;
                        } else {
                            clicks = 1;
                        }
                        click_time = now;
                        let x = mx - margin;
                        let y = my - margin;
                        selection = if clicks == 2 {
                            extend_to = ExtendTo::Word;
                            Selection::word_from_point(&paragraph, x, y)
                        } else if clicks == 3 {
                            extend_to = ExtendTo::Line;
                            Selection::line_from_point(&paragraph, x, y)
                        } else {
                            extend_to = ExtendTo::Point;
                            Selection::from_point(&paragraph, x, y)
                        };
                        selecting = true;
                        selection_changed = true;
                    } else {
                        selecting = false;
                    }
                }
                WindowEvent::ReceivedCharacter(ch) => {
                    // println!("got char {:?} [{}]", ch, ch as u32);
                    match ch as u32 {
                        8 | 13 | 127 => return,
                        _ => {}
                    }
                    if keymods.intersects(ModifiersState::CTRL | ModifiersState::LOGO) {
                        return;
                    }
                    if !selection.is_collapsed() {
                        if let Some(erase) = selection.erase(&paragraph) {
                            if let Some(offset) = doc.erase(erase) {
                                inserted = Some(offset);
                                if let Some(offs) = doc.insert(offset, ch) {
                                    inserted = Some(offs);
                                }
                                needs_update = true;
                            }
                        }
                    } else {
                        let place = selection.offset(&paragraph);
                        if let Some(offs) = doc.insert(place, ch) {
                            inserted = Some(offs);
                        }
                        needs_update = true;
                    }
                }
                WindowEvent::KeyboardInput { input, .. } => {
                    let window: &mut GlWindow = windows.get_mut(&window_id).unwrap();
                    use winit::event::{ElementState, VirtualKeyCode, VirtualKeyCode::*};
                    if input.state != ElementState::Pressed {
                        return;
                    }
                    if let Some(key) = input.virtual_keycode {
                        let shift = keymods.intersects(ModifiersState::SHIFT);
                        let ctrl = keymods.intersects(ModifiersState::CTRL);
                        let cmd = keymods.intersects(ModifiersState::LOGO);
                        window.request_redraw();
                        cursor_time = 0.;
                        cursor_on = true;
                        match key {
                            Return => {
                                let ch = '\n';
                                if !selection.is_collapsed() {
                                    if let Some(erase) = selection.erase(&paragraph) {
                                        if let Some(offset) = doc.erase(erase) {
                                            inserted = Some(offset);
                                            if let Some(offs) = doc.insert(offset, ch) {
                                                inserted = Some(offs);
                                            }
                                            needs_update = true;
                                        }
                                    }
                                } else {
                                    let place = selection.offset(&paragraph);
                                    if let Some(offs) = doc.insert(place, ch) {
                                        inserted = Some(offs);
                                    }
                                    needs_update = true;
                                }
                            }
                            Back => {
                                if let Some(erase) = selection.erase_previous(&paragraph) {
                                    if let Some(offset) = doc.erase(erase) {
                                        inserted = Some(offset);
                                        needs_update = true;
                                    }
                                }
                            }
                            Delete => {
                                if let Some(erase) = selection.erase(&paragraph) {
                                    if let Some(offset) = doc.erase(erase) {
                                        inserted = Some(offset);
                                        needs_update = true;
                                    }
                                }
                            }
                            C => {
                                if ctrl || cmd {
                                    let text =
                                        doc.get_selection(selection.normalized_range(&paragraph));
                                    clipboard.set_string_contents(text).ok();
                                }
                            }
                            V => {
                                if ctrl || cmd {
                                    if let Ok(text) = clipboard.get_string_contents() {
                                        if !selection.is_collapsed() {
                                            if let Some(erase) = selection.erase(&paragraph) {
                                                if let Some(offset) = doc.erase(erase) {
                                                    inserted = Some(offset);
                                                    if let Some(offs) =
                                                        doc.insert_str(offset, &text)
                                                    {
                                                        inserted = Some(offs);
                                                    }
                                                    needs_update = true;
                                                }
                                            }
                                        } else {
                                            let place = selection.offset(&paragraph);
                                            if let Some(offs) = doc.insert_str(place, &text) {
                                                inserted = Some(offs);
                                            }
                                            needs_update = true;
                                        }
                                    }
                                }
                            }
                            X => {
                                if ctrl || cmd {
                                    if !selection.is_collapsed() {
                                        let text =
                                            doc.get_selection(selection.normalized_range(&paragraph));
                                        clipboard.set_string_contents(text).ok();
                                        if let Some(erase) = selection.erase(&paragraph) {
                                            if let Some(offset) = doc.erase(erase) {
                                                inserted = Some(offset);
                                                needs_update = true;
                                            }
                                        }
                                    }
                                }
                            }
                            F1 => dark_mode = !dark_mode,
                            F2 => {
                                align = Alignment::Start;
                                size_changed = true;
                            }
                            F3 => {
                                align = Alignment::Middle;
                                size_changed = true;
                            }
                            F4 => {
                                align = Alignment::End;
                                size_changed = true;
                            }
                            F5 => {
                                //always_update = !always_update;
                            }
                            F7 => {
                                let mut clusters = Vec::new();
                                let mut u = 0;
                                for line in paragraph.lines() {
                                    for run in line.runs() {
                                        for (i, cluster) in run.visual_clusters().enumerate() {
                                            clusters.push((cluster, u, line.baseline()));
                                            u += 1;
                                        }
                                    }
                                }
                                let mut clusters2 = clusters.clone();
                                clusters2.sort_by(|a, b| a.0.offset().cmp(&b.0.offset()));
                                for (i, c2) in clusters2.iter().enumerate() {
                                    clusters[c2.1].1 = i;
                                }
                                let mut glyphs = Vec::new();
                                let mut x = 0.;
                                for cluster in &clusters {
                                    for mut glyph in cluster.0.glyphs() {
                                        glyph.x += x;
                                        glyph.y = cluster.2;
                                        x += glyph.advance;
                                        glyphs.push((cluster.1, glyph));
                                    }
                                }
                                let chars = doc.text.char_indices().collect::<Vec<_>>();
                                for (i, g) in glyphs.iter().enumerate() {
                                    if let Some((j, ch)) = chars.get(g.0).copied() {
                                        println!(
                                            "| {} | {} | {} | {} | {:.2}, {:.2} |",
                                            g.0, j, ch, g.1.id, g.1.x, g.1.y
                                        );
                                    }
                                }
                            }
                            Left => {
                                selection = if cmd {
                                    selection.home(&paragraph, shift)
                                } else {
                                    selection.previous(&paragraph, shift)
                                };
                                selection_changed = true;
                            }
                            Right => {
                                selection = if cmd {
                                    selection.end(&paragraph, shift)
                                } else {
                                    selection.next(&paragraph, shift)
                                };
                                selection_changed = true;
                            }
                            Home => {
                                selection = selection.home(&paragraph, shift);
                                selection_changed = true;
                            }
                            End => {
                                selection = selection.end(&paragraph, shift);
                                selection_changed = true;
                            }
                            Up => {
                                selection = selection.previous_line(&paragraph, shift);
                                selection_changed = true;
                            }
                            Down => {
                                selection = selection.next_line(&paragraph, shift);
                                selection_changed = true;
                            }
                            _ => {}
                        }
                    }
                }
                _ => (),
            },
            Event::MainEventsCleared => {
                // gl_window1.window.request_redraw();
            }
            Event::RedrawRequested(window_id) => {
                let window: &mut GlWindow = windows.get_mut(&window_id).unwrap();
                let cur_time = Instant::now();
                let dt = cur_time.duration_since(last_time).as_secs_f32();
                last_time = cur_time;
                frame_count += 1;
                total_time += dt;
                if total_time >= 1. {
                    use std::fmt::Write;
                    title.clear();
                    write!(
                        title,
                        "swash demo ({} fps)",
                        frame_count as f32 / total_time
                    )
                    .ok();
                    window.set_title(&title);
                    total_time = 0.;
                    frame_count = 0;
                }
                cursor_time += dt;
                if cursor_on {
                    if cursor_time > 0.5 {
                        cursor_time = 0.;
                        cursor_on = false;
                    }
                } else {
                    if cursor_time > 0.5 {
                        cursor_time = 0.;
                        cursor_on = true;
                    }
                }
                if first_run {
                    needs_update = true;
                }
                let window_size = window.inner_size();
                let w = window_size.width;
                let h = window_size.height;
                if always_update {
                    needs_update = true;
                }
                if needs_update {
                    let mut lb = layout_context.builder(Direction::LeftToRight, None, dpi);
                    doc.layout(&mut lb);
                    paragraph.clear();
                    lb.build_into(&mut paragraph);
                    println!("paragraph {:?}", &paragraph);
                    if first_run {
                        selection = Selection::from_point(&paragraph, 0., 0.);
                    }
                    first_run = false;
                    //paragraph.build_new_clusters();
                    needs_update = false;
                    size_changed = true;
                    selection_changed = true;
                }
                if size_changed {
                    let lw = w as f32 - margin * 2.;
                    paragraph.break_lines().break_remaining(lw, align);
                    size_changed = false;
                    selection_changed = true;
                }
                if let Some(offs) = inserted {
                    selection = Selection::from_offset(&paragraph, offs);
                }
                inserted = None;

                if selection_changed {
                    selection_rects.clear();
                    selection.regions_with(&paragraph, |r| {
                        selection_rects.push(r);
                    });
                    selection_changed = false;
                }

                let (fg, bg) = if dark_mode {
                    (color::WHITE_SMOKE, Color::new(20, 20, 20, 255))
                } else {
                    (color::BLACK, color::WHITE)
                };

                // draw_layout(&mut comp, &paragraph, margin, margin, 512., fg);

                // for r in &selection_rects {
                //     let rect = [r[0] + margin, r[1] + margin, r[2], r[3]];
                //     if dark_mode {
                //         comp.draw_rect(rect, 600., Color::new(38, 79, 120, 255));
                //     } else {
                //         comp.draw_rect(rect, 600., Color::new(179, 215, 255, 255));
                //     }
                // }

                // let (pt, ch, rtl) = selection.cursor(&paragraph);
                // if ch != 0. && cursor_on {
                //     let rect = [pt[0].round() + margin, pt[1].round() + margin, 1. * dpi, ch];
                //     comp.draw_rect(rect, 0.1, fg);
                // }
                window.redraw();
            }
            _ => (),
        }
    });

}

fn build_document() -> doc::Document {
    use layout::*;
    let mut db = doc::Document::builder();

    use SpanStyle as S;

    let underline = &[
        S::Underline(true),
        S::UnderlineOffset(Some(-1.)),
        S::UnderlineSize(Some(1.)),
    ];

    db.enter_span(&[
        S::family_list("times, georgia, serif"),
        S::Size(18.),
        S::features(&[("dlig", 1).into(), ("hlig", 1).into()][..]),
    ]);
    db.enter_span(&[S::LineSpacing(1.2)]);
    db.enter_span(&[S::family_list("baskerville, calibri, serif"), S::Size(22.)]);
    db.add_text("According to Wikipedia, the foremost expert on any subject,\n\n");
    db.leave_span();
    db.enter_span(&[S::Weight(Weight::BOLD)]);
    db.add_text("Typography");
    db.leave_span();
    db.add_text(" is the ");
    db.enter_span(&[S::Style(Style::Italic)]);
    db.add_text("art and technique");
    db.leave_span();
    db.add_text(" of arranging type to make ");
    db.enter_span(underline);
    db.add_text("written language");
    db.leave_span();
    db.add_text(" ");
    db.enter_span(underline);
    db.add_text("legible");
    db.leave_span();
    db.add_text(", ");
    db.enter_span(underline);
    db.add_text("readable");
    db.leave_span();
    db.add_text(" and ");
    db.enter_span(underline);
    db.add_text("appealing");
    db.leave_span();
    db.add_text(WIKI_TYPOGRAPHY_REST);
    db.enter_span(&[S::LineSpacing(1.)]);
    db.add_text(" Furthermore, العربية نص جميل. द क्विक ब्राउन फ़ॉक्स jumps over the lazy 🐕.\n\n");
    db.leave_span();
    db.enter_span(&[S::family_list("verdana, sans-serif"), S::LineSpacing(1.)]);
    db.add_text("A true ");
    db.enter_span(&[S::Size(48.)]);
    db.add_text("🕵🏽‍♀️");
    db.leave_span();
    db.add_text(" will spot the tricky selection in this BiDi text: ");
    db.enter_span(&[S::Size(22.)]);
    db.add_text("ניפגש ב09:35 בחוף הים");
    db.leave_span();
    db.build()
}

const WIKI_TYPOGRAPHY_REST: &'static str = " when displayed. The arrangement of type involves selecting typefaces, point sizes, line lengths, line-spacing (leading), and letter-spacing (tracking), and adjusting the space between pairs of letters (kerning). The term typography is also applied to the style, arrangement, and appearance of the letters, numbers, and symbols created by the process.";
