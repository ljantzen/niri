use std::cell::RefCell;

use ordered_float::NotNan;
use pangocairo::cairo::{self, ImageSurface};
use pangocairo::pango::FontDescription;
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::gles::GlesTexture;
use smithay::output::Output;
use smithay::reexports::gbm::Format as Fourcc;
use smithay::utils::{Point, Transform};

use crate::render_helpers::memory::MemoryBuffer;
use crate::render_helpers::primary_gpu_texture::PrimaryGpuTextureRenderElement;
use crate::render_helpers::renderer::NiriRenderer;
use crate::render_helpers::texture::{TextureBuffer, TextureRenderElement};
use crate::utils::{output_size, to_physical_precise_round};

const PADDING: i32 = 10;
const FONT: &str = "sans 14px";
const BORDER_RADIUS: f64 = 8.;
const BG_COLOR: (f64, f64, f64, f64) = (0.1, 0.1, 0.1, 0.85);
const TEXT_COLOR: (f64, f64, f64) = (1., 1., 1.);
const MARGIN_TOP: i32 = 12;

struct CachedBuffer {
    text: String,
    buffer: TextureBuffer<GlesTexture>,
}

pub struct OverviewFilterUi {
    /// Cached GPU textures per scale, invalidated when text changes.
    ///
    /// Uses interior mutability so the render method can be called on `&self`.
    buffers: RefCell<Vec<(NotNan<f64>, CachedBuffer)>>,
}

impl OverviewFilterUi {
    pub fn new() -> Self {
        Self {
            buffers: RefCell::new(Vec::new()),
        }
    }

    /// Returns a render element for the filter bar on the given output, or `None` if the filter
    /// text is empty.
    pub fn render<R: NiriRenderer>(
        &self,
        renderer: &mut R,
        output: &Output,
        text: &str,
    ) -> Option<PrimaryGpuTextureRenderElement> {
        if text.is_empty() {
            return None;
        }

        let scale = output.current_scale().fractional_scale();
        let scale_key = NotNan::new(scale).ok()?;
        let output_size = output_size(output);

        let mut buffers = self.buffers.borrow_mut();

        // Find or create the cached buffer for this scale.
        let pos = buffers.iter().position(|(s, _)| *s == scale_key);
        let needs_update = pos
            .map(|i| buffers[i].1.text.as_str() != text)
            .unwrap_or(true);

        let idx = if needs_update {
            let memory_buf = match render_memory_buffer(text, scale) {
                Ok(buf) => buf,
                Err(err) => {
                    warn!("error rendering overview filter bar: {err:?}");
                    return None;
                }
            };
            let texture =
                TextureBuffer::from_memory_buffer(renderer.as_gles_renderer(), &memory_buf).ok()?;
            let cached = CachedBuffer {
                text: text.to_owned(),
                buffer: texture,
            };
            if let Some(i) = pos {
                buffers[i].1 = cached;
                i
            } else {
                buffers.push((scale_key, cached));
                buffers.len() - 1
            }
        } else {
            pos.expect("pos is Some when needs_update is false")
        };

        let buf = &buffers[idx].1.buffer;

        let size = buf.logical_size();

        // Center horizontally, place near the top.
        let x = (output_size.w - size.w) / 2.;
        let y = f64::from(MARGIN_TOP);
        let location = Point::from((x, y));
        let location = location.to_physical_precise_round(scale).to_logical(scale);

        let elem = TextureRenderElement::from_texture_buffer(
            buf.clone(),
            location,
            1.,
            None,
            None,
            Kind::Unspecified,
        );
        Some(PrimaryGpuTextureRenderElement(elem))
    }

    /// Clears all cached textures (e.g., when scale changes or overview closes).
    pub fn invalidate(&mut self) {
        self.buffers.borrow_mut().clear();
    }
}

fn render_memory_buffer(text: &str, scale: f64) -> anyhow::Result<MemoryBuffer> {
    let padding: i32 = to_physical_precise_round(scale, PADDING);

    let mut font = FontDescription::from_string(FONT);
    font.set_absolute_size(to_physical_precise_round(scale, font.size()));

    // First pass: measure text.
    let dummy = ImageSurface::create(cairo::Format::ARgb32, 0, 0)?;
    let cr = cairo::Context::new(&dummy)?;
    let layout = pangocairo::functions::create_layout(&cr);
    layout.context().set_round_glyph_positions(false);
    layout.set_font_description(Some(&font));
    layout.set_text(text);

    let (text_w, text_h) = layout.pixel_size();
    let width = text_w + padding * 2;
    let height = text_h + padding * 2;

    // Second pass: render.
    let surface = ImageSurface::create(cairo::Format::ARgb32, width, height)?;
    let cr = cairo::Context::new(&surface)?;

    // Rounded rectangle background.
    let r = (BORDER_RADIUS * scale).max(0.);
    let w = f64::from(width);
    let h = f64::from(height);
    cr.new_sub_path();
    cr.arc(w - r, r, r, -std::f64::consts::FRAC_PI_2, 0.);
    cr.arc(w - r, h - r, r, 0., std::f64::consts::FRAC_PI_2);
    cr.arc(
        r,
        h - r,
        r,
        std::f64::consts::FRAC_PI_2,
        std::f64::consts::PI,
    );
    cr.arc(
        r,
        r,
        r,
        std::f64::consts::PI,
        3. * std::f64::consts::FRAC_PI_2,
    );
    cr.close_path();
    let (br, bg, bb, ba) = BG_COLOR;
    cr.set_source_rgba(br, bg, bb, ba);
    cr.fill()?;

    // Text.
    cr.move_to(f64::from(padding), f64::from(padding));
    let layout = pangocairo::functions::create_layout(&cr);
    layout.context().set_round_glyph_positions(false);
    layout.set_font_description(Some(&font));
    layout.set_text(text);
    let (tr, tg, tb) = TEXT_COLOR;
    cr.set_source_rgb(tr, tg, tb);
    pangocairo::functions::show_layout(&cr, &layout);
    drop(cr);

    let data = surface.take_data().unwrap();
    let buffer = MemoryBuffer::new(
        data.to_vec(),
        Fourcc::Argb8888,
        (width, height),
        scale,
        Transform::Normal,
    );
    Ok(buffer)
}
