//! A frosted backdrop for the panel on Linux, standing in for the macOS
//! popover material: GNOME's compositor offers applications no blur of what
//! lies behind a window. The backdrop is the desktop wallpaper as GNOME draws
//! it on the panel's monitor, blurred and saturated; the web UI aligns it with
//! the window and tints it. Other windows under the panel do not show through.

use gdk_pixbuf::{gio, glib, prelude::*, Colorspace, Pixbuf};
use serde::Serialize;
use std::{
    sync::{Arc, Mutex},
    time::SystemTime,
};
use tauri::{Emitter, Monitor, WebviewWindow};

/// The backdrop has this fraction of the monitor's logical resolution; the
/// blur hides the upscaling.
const DOWNSCALE: f64 = 4.0;
/// Box blur radius in backdrop pixels. Three passes approximate a Gaussian of
/// about 30 logical points, like the macOS popover material.
const BLUR_RADIUS: usize = 6;
const SATURATION: f32 = 1.6;

/// What the backdrop is rendered from; a change renders it again.
#[derive(Clone, Debug, PartialEq)]
struct Source {
    uri: String,
    options: String,
    color: String,
    modified: Option<SystemTime>,
    width: i32,
    height: i32,
}

/// The window's place on its monitor in logical points. `version` names the
/// rendered backdrop, 0 when there is none.
#[derive(Clone, Copy, Default, Serialize)]
struct Geometry {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    version: u64,
}

struct Cache {
    source: Option<Source>,
    rendering: Option<Source>,
    png: Option<Arc<Vec<u8>>>,
    version: u64,
    geometry: Geometry,
}

impl Cache {
    /// The version the UI is told, 0 without a rendered backdrop.
    fn version(&self) -> u64 {
        if self.png.is_some() {
            self.version
        } else {
            0
        }
    }
}

static CACHE: Mutex<Cache> = Mutex::new(Cache {
    source: None,
    rendering: None,
    png: None,
    version: 0,
    geometry: Geometry {
        x: 0.0,
        y: 0.0,
        width: 0.0,
        height: 0.0,
        version: 0,
    },
});

/// The rendered backdrop as PNG.
pub fn png() -> Option<Arc<Vec<u8>>> {
    CACHE.lock().unwrap().png.clone()
}

/// Tells the UI where the window at physical `(x, y)` lies on `monitor`,
/// rendering the backdrop first when the wallpaper or monitor changed.
pub fn update(w: &WebviewWindow, monitor: &Monitor, x: f64, y: f64) {
    let scale = monitor.scale_factor();
    let (position, size) = (monitor.position(), monitor.size());
    let (width, height) = (size.width as f64 / scale, size.height as f64 / scale);
    let geometry = Geometry {
        x: (x - position.x as f64) / scale,
        y: (y - position.y as f64) / scale,
        width,
        height,
        version: 0,
    };
    let source = Source::current(
        (width / DOWNSCALE).round().max(1.0) as i32,
        (height / DOWNSCALE).round().max(1.0) as i32,
    );
    let mut cache = CACHE.lock().unwrap();
    cache.geometry = geometry;
    if source == cache.source {
        let version = cache.version();
        drop(cache);
        let _ = w.emit(
            "panel-backdrop",
            Geometry {
                version,
                ..geometry
            },
        );
        return;
    }
    // A render under way emits the latest geometry when it finishes.
    if source == cache.rendering {
        return;
    }
    cache.rendering = source.clone();
    drop(cache);
    let w = w.clone();
    std::thread::spawn(move || {
        let png = source.as_ref().and_then(render);
        let mut cache = CACHE.lock().unwrap();
        if cache.rendering != source {
            return;
        }
        cache.rendering = None;
        cache.source = source;
        cache.version += 1;
        cache.png = png.map(Arc::new);
        let version = cache.version();
        let geometry = Geometry {
            version,
            ..cache.geometry
        };
        drop(cache);
        let _ = w.emit("panel-backdrop", geometry);
    });
}

fn settings(schema: &str) -> Option<gio::Settings> {
    let schema = gio::SettingsSchemaSource::default()?.lookup(schema, true)?;
    Some(gio::Settings::new_full(
        &schema,
        None::<&gio::SettingsBackend>,
        None,
    ))
}

impl Source {
    /// GNOME's current wallpaper, at `width` × `height` backdrop pixels.
    fn current(width: i32, height: i32) -> Option<Source> {
        let background = settings("org.gnome.desktop.background")?;
        // GNOME 42 and later show a separate wallpaper in the dark style.
        let dark = settings("org.gnome.desktop.interface")
            .filter(|s| {
                s.settings_schema()
                    .is_some_and(|s| s.has_key("color-scheme"))
            })
            .is_some_and(|s| s.string("color-scheme") == "prefer-dark");
        let has_dark = background
            .settings_schema()
            .is_some_and(|s| s.has_key("picture-uri-dark"));
        let key = if dark && has_dark {
            "picture-uri-dark"
        } else {
            "picture-uri"
        };
        let uri = background.string(key).to_string();
        let modified = gio::File::for_uri(&uri)
            .path()
            .and_then(|path| path.metadata().ok()?.modified().ok());
        Some(Source {
            uri,
            options: background.string("picture-options").to_string(),
            color: background.string("primary-color").to_string(),
            modified,
            width,
            height,
        })
    }
}

/// The wallpaper scaled as GNOME's default "zoom" (cover) or "stretched"
/// options draw it; other options are approximated by "zoom".
fn picture(source: &Source) -> Option<Pixbuf> {
    if source.options == "none" {
        return None;
    }
    let (w, h) = (source.width, source.height);
    let path = gio::File::for_uri(&source.uri).path()?;
    let (_, iw, ih) = Pixbuf::file_info(&path)?;
    if iw <= 0 || ih <= 0 {
        return None;
    }
    let (sw, sh) = if source.options == "stretched" {
        (w, h)
    } else {
        let s = (w as f64 / iw as f64).max(h as f64 / ih as f64);
        (
            ((iw as f64 * s).ceil() as i32).max(w),
            ((ih as f64 * s).ceil() as i32).max(h),
        )
    };
    let full = Pixbuf::from_file_at_scale(&path, sw, sh, false).ok()?;
    let (sw, sh) = (full.width(), full.height());
    if sw < w || sh < h {
        return None;
    }
    Some(full.new_subpixbuf((sw - w) / 2, (sh - h) / 2, w, h))
}

/// The RGB channels of `pixbuf`, row by row, as floats.
fn pixels(pixbuf: &Pixbuf) -> Vec<f32> {
    let (w, h) = (pixbuf.width() as usize, pixbuf.height() as usize);
    let (stride, channels) = (pixbuf.rowstride() as usize, pixbuf.n_channels() as usize);
    let bytes = pixbuf.read_pixel_bytes();
    let mut out = Vec::with_capacity(w * h * 3);
    for y in 0..h {
        for x in 0..w {
            let i = y * stride + x * channels;
            out.extend(bytes[i..i + 3].iter().map(|&c| c as f32));
        }
    }
    out
}

/// "#rrggbb" as a flat color.
fn solid(color: &str, w: usize, h: usize) -> Option<Vec<f32>> {
    let hex = color.strip_prefix('#').filter(|hex| hex.len() == 6)?;
    let channel = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok().map(f32::from);
    let rgb = [channel(0)?, channel(2)?, channel(4)?];
    Some(rgb.iter().copied().cycle().take(w * h * 3).collect())
}

/// One box blur pass along rows or columns, clamping at the edges.
fn box_pass(src: &[f32], dst: &mut [f32], w: usize, h: usize, horizontal: bool) {
    let (lines, len) = if horizontal { (h, w) } else { (w, h) };
    let at = |line: usize, i: usize| {
        if horizontal {
            (line * w + i) * 3
        } else {
            (i * w + line) * 3
        }
    };
    let r = BLUR_RADIUS as isize;
    let n = (2 * r + 1) as f32;
    for line in 0..lines {
        let clamp = |i: isize| at(line, i.clamp(0, len as isize - 1) as usize);
        let mut sum = [0.0f32; 3];
        for i in -r..=r {
            let p = clamp(i);
            for c in 0..3 {
                sum[c] += src[p + c];
            }
        }
        for i in 0..len {
            let p = at(line, i);
            for c in 0..3 {
                dst[p + c] = sum[c] / n;
            }
            let (add, remove) = (clamp(i as isize + r + 1), clamp(i as isize - r));
            for c in 0..3 {
                sum[c] += src[add + c] - src[remove + c];
            }
        }
    }
}

fn render(source: &Source) -> Option<Vec<u8>> {
    let (w, h) = (source.width as usize, source.height as usize);
    let mut rgb = match picture(source) {
        Some(pixbuf) => pixels(&pixbuf),
        None => solid(&source.color, w, h)?,
    };
    let mut scratch = vec![0.0; rgb.len()];
    for _ in 0..3 {
        box_pass(&rgb, &mut scratch, w, h, true);
        box_pass(&scratch, &mut rgb, w, h, false);
    }
    let bytes: Vec<u8> = rgb
        .chunks_exact(3)
        .flat_map(|p| {
            let luma = 0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2];
            p.iter()
                .map(move |&c| (luma + (c - luma) * SATURATION).round().clamp(0.0, 255.0) as u8)
        })
        .collect();
    let pixbuf = Pixbuf::from_bytes(
        &glib::Bytes::from_owned(bytes),
        Colorspace::Rgb,
        false,
        8,
        w as i32,
        h as i32,
        w as i32 * 3,
    );
    pixbuf.save_to_bufferv("png", &[]).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blur_keeps_a_flat_color_and_spreads_an_edge() {
        let (w, h) = (20, 3);
        let flat = solid("#336699", w, h).unwrap();
        let mut out = vec![0.0; flat.len()];
        box_pass(&flat, &mut out, w, h, true);
        assert!(out.iter().zip(&flat).all(|(a, b)| (a - b).abs() < 1e-3));

        let edge: Vec<f32> = (0..w * h)
            .flat_map(|i| [if i % w < w / 2 { 0.0 } else { 255.0 }; 3])
            .collect();
        box_pass(&edge, &mut out, w, h, true);
        assert_eq!(out[0], 0.0);
        let mid = (w / 2) * 3;
        assert!(out[mid] > 0.0 && out[mid] < 255.0);
        assert_eq!(out[(w - 1) * 3], 255.0);
    }

    #[test]
    fn solid_parses_gnome_colors() {
        assert_eq!(solid("#023c88", 1, 1), Some(vec![2.0, 60.0, 136.0]));
        assert_eq!(solid("023c88", 1, 1), None);
        assert_eq!(solid("#xyzxyz", 1, 1), None);
    }
}
