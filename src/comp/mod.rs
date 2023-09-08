pub use swash;

pub mod color;
pub mod shaders;
pub mod text;

mod batch;
mod image_cache;

pub use batch::{Command, DisplayList, Rect, Vertex, Pipeline};
pub use image_cache::{
    Epoch, AddImage, ImageData, ImageId, ImageLocation, TextureEvent, TextureId,
};

use batch::BatchManager;
use color::Color;
use image_cache::{GlyphCache, ImageCache};
use text::*;

use std::borrow::Borrow;

pub struct Compositor {
    image_cache: ImageCache,
    glyph_cache: GlyphCache,
    batch_manager: BatchManager,
    epoch: Epoch,
    intercepts: Vec<(f32, f32)>,
}

impl Compositor {
    /// Creates a new compositor.
    pub fn new(max_texture_size: u16) -> Self {
        Self {
            image_cache: ImageCache::new(max_texture_size),
            glyph_cache: GlyphCache::new(),
            batch_manager: BatchManager::new(),
            epoch: Epoch(0),
            intercepts: Vec::new(),
        }
    }

    /// Advances the epoch for the compositor and clears all batches.
    pub fn begin(&mut self) {
        self.glyph_cache.prune(self.epoch, &mut self.image_cache);
        self.epoch.0 += 1;
        self.batch_manager.reset();
    }

    /// Builds a display list for the current batched geometry and enumerates
    /// all texture events with the specified closure.
    pub fn finish(&mut self, list: &mut DisplayList, events: impl FnMut(TextureEvent)) {
        self.image_cache.drain_events(events);
        self.batch_manager.build_display_list(list);
    }
}

/// Image management.
impl Compositor {
    /// Adds an image to the compositor.
    pub fn add_image(&mut self, request: AddImage) -> Option<ImageId> {
        self.image_cache.allocate(self.epoch, request)
    }

    /// Returns the image associated with the specified identifier.
    pub fn get_image(&mut self, image: ImageId) -> Option<ImageLocation> {
        self.image_cache.get(self.epoch, image)
    }

    /// Removes the image from the compositor.
    pub fn remove_image(&mut self, image: ImageId) -> bool {
        self.image_cache.deallocate(image).is_some()
    }
}

/// Drawing.
impl Compositor {
    /// Draws a rectangle with the specified depth and color.
    pub fn draw_rect(&mut self, rect: impl Into<Rect>, depth: f32, color: Color) {
        self.batch_manager.add_rect(&rect.into(), depth, color);
    }

    /// Draws an image with the specified rectangle, depth and color.
    pub fn draw_image(&mut self, rect: impl Into<Rect>, depth: f32, color: Color, image: ImageId) {
        if let Some(img) = self.image_cache.get(self.epoch, image) {
            self.batch_manager.add_image_rect(
                &rect.into(),
                depth,
                color,
                &[img.min.0, img.min.1, img.max.0, img.max.1],
                img.texture_id,
                image.has_alpha(),
            );
        }
    }

    /// Draws a text run.
    pub fn draw_glyphs<I>(&mut self, rect: impl Into<Rect>, depth: f32, style: &TextRunStyle, glyphs: I)
    where
        I: Iterator,
        I::Item: Borrow<Glyph>,
    {
        let rect = rect.into();
        let (underline, underline_offset, underline_size, underline_color) = match style.underline {
            Some(underline) => {
                (true, underline.offset.round() as i32, underline.size.round().max(1.), underline.color)
            }
            _ => (false, 0, 0., Color::default())
        };
        if underline {
            self.intercepts.clear();
        }
        let mut session = self.glyph_cache.session(
            self.epoch,
            &mut self.image_cache,
            style.font,
            style.font_coords,
            style.font_size,
        );        
        let subpx_bias = (0.125, 0.);    
        let color = style.color;    
        let x = rect.x;
        for g in glyphs {           
            let glyph = g.borrow();
            let entry = session.get(glyph.id, glyph.x, glyph.y);
            if let Some(entry) = entry {
                if let Some(img) = session.get_image(entry.image) {
                    let gx = (glyph.x + subpx_bias.0).floor() + entry.left as f32;
                    let gy = (glyph.y + subpx_bias.1).floor() - entry.top as f32;
                    if entry.is_bitmap {
                        self.batch_manager.add_image_rect(
                            &Rect::new(gx, gy, entry.width as f32, entry.height as f32),
                            depth,
                            color::WHITE,
                            &[img.min.0, img.min.1, img.max.0, img.max.1],
                            img.texture_id,
                            entry.image.has_alpha(),
                        );
                    } else {
                        self.batch_manager.add_mask_rect(
                            &Rect::new(gx, gy, entry.width as f32, entry.height as f32),
                            depth,
                            color,
                            &[img.min.0, img.min.1, img.max.0, img.max.1],
                            img.texture_id,
                            true,
                        );
                    }
                    if underline {
                        if entry.top - underline_offset < entry.height as i32 {
                            if let Some(mut desc_ink) = entry.desc.range() {
                                desc_ink.0 += gx;
                                desc_ink.1 += gx;
                                self.intercepts.push(desc_ink);
                            }
                        }
                    }                                
                }
            }
        }
        if underline {
            for range in self.intercepts.iter_mut() {
                range.0 -= 1.;
                range.1 += 1.;
            }
            let mut ux = x;
            let uy = style.baseline - underline_offset as f32;
            for range in self.intercepts.iter() {
                if ux < range.0 {
                    self.batch_manager.add_rect(&Rect::new(ux, uy, range.0 - ux, underline_size as f32), depth, underline_color);
                }
                ux = range.1;
            }
            let end = x + rect.width;
            if ux < end {
                self.draw_rect(Rect::new(ux, uy, end - ux, underline_size), depth, underline_color);
            }
        }        
    }
}

