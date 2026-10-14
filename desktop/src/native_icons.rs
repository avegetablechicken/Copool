//! Render the same SF Symbols as SwiftUI using the user's AppKit catalog.
//! Each CSS size has its own 1x/2x/3x representation: no enlarged thumbnails,
//! no non-uniform SVG scaling, and no exported Apple symbols on other systems.
use std::collections::BTreeMap;

#[derive(Default)]
pub struct NativeIcons(pub BTreeMap<String, Vec<String>>);

#[tauri::command]
pub fn native_symbols(icons: tauri::State<'_, NativeIcons>) -> BTreeMap<String, Vec<String>> {
    icons.0.clone()
}

#[cfg(not(target_os = "macos"))]
pub fn load() -> Result<NativeIcons, String> {
    Ok(NativeIcons::default())
}

#[cfg(target_os = "macos")]
pub fn load() -> Result<NativeIcons, String> {
    use base64::{engine::general_purpose::STANDARD, Engine};
    let mut icons = BTreeMap::new();
    let names = [
        ("network", "network"),
        ("plus", "plus"),
        ("import", "square.and.arrow.down"),
        ("reauthenticate", "person.crop.circle.badge.exclamationmark"),
        ("refresh", "arrow.clockwise"),
        ("renewal", "clock.arrow.circlepath"),
        ("chevron-up", "chevron.up"),
        ("chevron-down", "chevron.down"),
        ("chevron-left", "chevron.left"),
        ("trash", "trash"),
        ("switch", "arrow.left.arrow.right.circle.fill"),
        ("copy", "doc.on.doc"),
        ("eye", "eye"),
        ("close", "xmark"),
        ("server", "server.rack"),
        ("folder", "folder"),
        ("warning", "exclamationmark.triangle"),
    ];
    for (key, symbol) in names {
        // The Coport-style panel draws icons at 11–14 pt.
        for size in 11..=14 {
            let mut variants = Vec::new();
            for scale in 1..=3 {
                let bytes = macos::render(symbol, size, scale, 0.23)?;
                variants.push(format!("data:image/png;base64,{}", STANDARD.encode(bytes)));
            }
            icons.insert(format!("{key}:{size}"), variants);
        }
    }
    Ok(NativeIcons(icons))
}

#[cfg(target_os = "macos")]
pub fn tray() -> Result<tauri::image::Image<'static>, String> {
    // Same figure.pool.swim and black weight as CopoolApp.makeMenuBarSymbolImage.
    let png = macos::render("figure.pool.swim", 18, 2, 0.62)?;
    tauri::image::Image::from_bytes(&png).map_err(|e| e.to_string())
}

#[cfg(target_os = "macos")]
mod macos {
    use objc2::{AnyThread, MainThreadMarker};
    use objc2_app_kit::{
        NSBitmapImageFileType, NSBitmapImageRep, NSCompositingOperation, NSDeviceRGBColorSpace,
        NSGraphicsContext, NSImage, NSImageSymbolConfiguration,
    };
    use objc2_foundation::{NSDictionary, NSPoint, NSRect, NSSize, NSString};

    pub fn render(name: &str, size: usize, scale: usize, weight: f64) -> Result<Vec<u8>, String> {
        let _main = MainThreadMarker::new().ok_or("SF Symbols must render on the main thread")?;
        let symbol = NSImage::imageWithSystemSymbolName_accessibilityDescription(
            &NSString::from_str(name),
            None,
        )
        .ok_or_else(|| format!("System symbol is unavailable: {name}"))?;
        let configuration = NSImageSymbolConfiguration::configurationWithPointSize_weight(
            (size as f64 * 0.85).round(),
            weight,
        );
        let symbol = symbol
            .imageWithSymbolConfiguration(&configuration)
            .unwrap_or(symbol);
        let pixels = size * scale;
        // AppKit owns the allocated, interleaved RGBA buffer. Fixed dimensions,
        // 8-bit samples and explicit row stride bound all subsequent drawing.
        let bitmap = unsafe {
            NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel(
                NSBitmapImageRep::alloc(), std::ptr::null_mut(), pixels as isize,
                pixels as isize, 8, 4, true, false, NSDeviceRGBColorSpace,
                (pixels * 4) as isize, 32,
            )
        }.ok_or("Cannot allocate symbol bitmap")?;
        let context = NSGraphicsContext::graphicsContextWithBitmapImageRep(&bitmap)
            .ok_or("Cannot create symbol graphics context")?;
        let natural = symbol.size();
        if natural.width <= 0.0 || natural.height <= 0.0 {
            return Err("Empty symbol".into());
        }
        let fit = (size as f64 / natural.width.max(natural.height)).min(1.0);
        let width = (natural.width * fit * scale as f64).round();
        let height = (natural.height * fit * scale as f64).round();
        let rect = NSRect::new(
            NSPoint::new(
                ((pixels as f64 - width) / 2.0).floor(),
                ((pixels as f64 - height) / 2.0).floor(),
            ),
            NSSize::new(width, height),
        );
        NSGraphicsContext::saveGraphicsState_class();
        NSGraphicsContext::setCurrentContext(Some(&context));
        symbol.drawInRect_fromRect_operation_fraction(
            rect,
            NSRect::ZERO,
            NSCompositingOperation::Copy,
            1.0,
        );
        NSGraphicsContext::restoreGraphicsState_class();
        // An empty properties dictionary is valid for every bitmap file type.
        let png = unsafe {
            bitmap.representationUsingType_properties(
                NSBitmapImageFileType::PNG,
                &NSDictionary::new(),
            )
        }
        .ok_or("Cannot encode symbol PNG")?;
        Ok(png.to_vec())
    }
}
