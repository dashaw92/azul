use std::{
    fmt,
    path::PathBuf,
    io::Error as IoError,
};
use webrender::api::{RenderApi as WrRenderApi};
use azul_core::app_resources::{ResourceUpdate, FontImageApi};
#[cfg(feature = "image_loading")]
pub use image::{ImageError, DynamicImage, GenericImageView};
pub use azul_core::{
    app_resources::{
        AppResources, Au, ImmediateFontId, LoadedFont, RawImageFormat,
        FontKey, FontInstanceKey, ImageKey, ImageSource, FontSource,
        RawImage, CssFontId, CssImageId, TextCache, TextId, ImageId, FontId,
        ImageInfo, IdNamespace, ImageData, ImageDescriptor,
    },
    callbacks::PipelineId,
    id_tree::NodeDataContainer,
    dom::NodeData,
};

#[derive(Debug)]
pub enum ImageReloadError {
    Io(IoError, PathBuf),
    #[cfg(feature = "image_loading")]
    DecodingError(ImageError),
    #[cfg(not(feature = "image_loading"))]
    DecodingModuleNotActive,
}

impl Clone for ImageReloadError {
    fn clone(&self) -> Self {
        use self::ImageReloadError::*;
        match self {
            Io(err, path) => Io(IoError::new(err.kind(), "Io Error"), path.clone()),
            #[cfg(feature = "image_loading")]
            DecodingError(e) => DecodingError(e.clone()),
            #[cfg(not(feature = "image_loading"))]
            DecodingModuleNotActive => DecodingModuleNotActive,
        }
    }
}

impl fmt::Display for ImageReloadError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        use self::ImageReloadError::*;
        match &self {
            Io(err, path_buf) => write!(f, "Could not load \"{}\" - IO error: {}", path_buf.as_path().to_string_lossy(), err),
            #[cfg(feature = "image_loading")]
            DecodingError(err) => write!(f, "Image decoding error: \"{}\"", err),
            #[cfg(not(feature = "image_loading"))]
            DecodingModuleNotActive => write!(f, "Found decoded image, but crate was not compiled with --features=\"image_loading\""),
        }
    }
}

#[derive(Debug)]
pub enum FontReloadError {
    Io(IoError, PathBuf),
    FontNotFound(String),
}

impl Clone for FontReloadError {
    fn clone(&self) -> Self {
        use self::FontReloadError::*;
        match self {
            Io(err, path) => Io(IoError::new(err.kind(), "Io Error"), path.clone()),
            FontNotFound(id) => FontNotFound(id.clone()),
        }
    }
}

impl_display!(FontReloadError, {
    Io(err, path_buf) => format!("Could not load \"{}\" - IO error: {}", path_buf.as_path().to_string_lossy(), err),
    FontNotFound(id) => format!("Could not locate system font: \"{}\" found", id),
});

/// Wrapper struct because it's not possible to implement traits on foreign types
pub(crate) struct WrApi {
    pub api: WrRenderApi,
}

impl FontImageApi for WrApi {
    fn new_image_key(&self) -> ImageKey {
        use crate::wr_translate::translate_image_key_wr;
        translate_image_key_wr(self.api.generate_image_key())
    }
    fn new_font_key(&self) -> FontKey {
        use crate::wr_translate::translate_font_key_wr;
        translate_font_key_wr(self.api.generate_font_key())
    }
    fn new_font_instance_key(&self) -> FontInstanceKey {
        use crate::wr_translate::translate_font_instance_key_wr;
        translate_font_instance_key_wr(self.api.generate_font_instance_key())
    }
    fn update_resources(&self, updates: Vec<ResourceUpdate>) {
        use crate::wr_translate::wr_translate_resource_update;
        let wr_updates = updates.into_iter().map(wr_translate_resource_update).collect();
        self.api.update_resources(wr_updates);
    }
    fn flush_scene_builder(&self) {
        self.api.flush_scene_builder();
    }
}

/// Returns the **decoded** bytes of the image + the descriptor (contains width / height).
/// Returns an error if the data is encoded, but the crate wasn't built with `--features="image_loading"`
#[allow(unused_variables)]
pub fn image_source_get_bytes(image_source: &ImageSource) -> Option<(ImageData, ImageDescriptor)> {

    fn image_source_get_bytes_inner(image_source: &ImageSource)
    -> Result<(ImageData, ImageDescriptor), ImageReloadError>
    {
        use std::sync::Arc;
        match image_source {
            ImageSource::Embedded(bytes) => {
                #[cfg(feature = "image_loading")] {
                    decode_image_data(bytes.to_vec()).map_err(|e| ImageReloadError::DecodingError(e))
                }
                #[cfg(not(feature = "image_loading"))] {
                    Err(ImageReloadError::DecodingModuleNotActive)
                }
            },
            ImageSource::Raw(raw_image) => {
                use azul_core::app_resources::is_image_opaque;
                let is_opaque = is_image_opaque(raw_image.data_format, &raw_image.pixels[..]);
                let descriptor = ImageDescriptor {
                    format: raw_image.data_format,
                    dimensions: raw_image.image_dimensions,
                    stride: None,
                    offset: 0,
                    is_opaque,
                    allow_mipmaps: true,
                };
                let data = ImageData::Raw(Arc::new(raw_image.pixels.clone()));
                Ok((data, descriptor))
            },
            ImageSource::File(file_path) => {
                #[cfg(feature = "image_loading")] {
                    use std::fs;
                    let bytes = fs::read(file_path).map_err(|e| ImageReloadError::Io(e, file_path.clone()))?;
                    decode_image_data(bytes).map_err(|e| ImageReloadError::DecodingError(e))
                }
                #[cfg(not(feature = "image_loading"))] {
                    Err(ImageReloadError::DecodingModuleNotActive)
                }
            },
        }
    }

    match image_source_get_bytes_inner(image_source) {
        Ok(o) => Some(o),
        Err(e) => {
            #[cfg(feature = "logging")] {
                error!("Could not load image source \"{:?}\", error: {}", image_source, e);
            }
            None
        }
    }
}

pub fn font_source_get_bytes(font_source: &FontSource) -> Option<(Vec<u8>, i32)> {

    /// Returns the bytes of the font (loads the font from the system in case it is a `FontSource::System` font).
    /// Also returns the index into the font (in case the font is a font collection).
    fn font_source_get_bytes_inner(font_source: &FontSource) -> Result<(Vec<u8>, i32), FontReloadError> {
        use std::fs;
        match font_source {
            FontSource::Embedded(bytes) => Ok((bytes.to_vec(), 0)),
            FontSource::File(file_path) => {
                fs::read(file_path)
                .map_err(|e| FontReloadError::Io(e, file_path.clone()))
                .map(|f| (f, 0))
            },
            FontSource::System(id) => load_system_font(id).ok_or(FontReloadError::FontNotFound(id.clone())),
        }
    }

    match font_source_get_bytes_inner(font_source) {
        Ok(o) => Some(o),
        Err(e) => {
            #[cfg(feature = "logging")] {
                error!("Could not load font source \"{:?}\", error: {}", font_source, e);
            }
            None
        }
    }
}

#[cfg(feature = "image_loading")]
fn decode_image_data(image_data: Vec<u8>) -> Result<(ImageData, ImageDescriptor), ImageError> {
    use image; // the crate

    let image_format = image::guess_format(&image_data)?;
    let decoded = image::load_from_memory_with_format(&image_data, image_format)?;
    Ok(prepare_image(decoded)?)
}

/// Returns the font + the index of the font (in case the font is a collection)
fn load_system_font(id: &str) -> Option<(Vec<u8>, i32)> {
    use font_loader::system_fonts::{self, FontPropertyBuilder};

    let font_builder = match id {
        "monospace" => {
            #[cfg(target_os = "linux")] {
                let native_monospace_font = linux_get_native_font(LinuxNativeFontType::Monospace);
                FontPropertyBuilder::new().family(&native_monospace_font)
            }
            #[cfg(not(target_os = "linux"))] {
                FontPropertyBuilder::new().monospace()
            }
        },
        "fantasy" => FontPropertyBuilder::new().oblique(),
        "sans-serif" => {
            #[cfg(target_os = "mac_os")] {
                FontPropertyBuilder::new().family("Helvetica")
            }
            #[cfg(target_os = "linux")] {
                let native_sans_serif_font = linux_get_native_font(LinuxNativeFontType::SansSerif);
                FontPropertyBuilder::new().family(&native_sans_serif_font)
            }
            #[cfg(all(not(target_os = "linux"), not(target_os = "mac_os")))] {
                FontPropertyBuilder::new().family("Segoe UI")
            }
        },
        "serif" => {
            FontPropertyBuilder::new().family("Times New Roman")
        },
        other => FontPropertyBuilder::new().family(other)
    };

    system_fonts::get(&font_builder.build())
}

/// Return the native fonts
#[cfg(target_os = "linux")]
enum LinuxNativeFontType { SansSerif, Monospace }

#[cfg(target_os = "linux")]
fn linux_get_native_font(font_type: LinuxNativeFontType) -> String {

    use std::process::Command;
    use self::LinuxNativeFontType::*;

    let font_name = match font_type {
        SansSerif => "font-name",
        Monospace => "monospace-font-name",
    };

    let fallback_font_name = match font_type {
        SansSerif => "Ubuntu",
        Monospace => "Ubuntu Mono",
    };

    // Execute "gsettings get org.gnome.desktop.interface font-name" and parse the output
    let gsetting_cmd_result =
        Command::new("gsettings")
            .arg("get")
            .arg("org.gnome.desktop.interface")
            .arg(font_name)
            .output()
            .ok().map(|output| output.stdout)
            .and_then(|stdout_bytes| String::from_utf8(stdout_bytes).ok())
            .map(|stdout_string| stdout_string.lines().collect::<String>());

    match &gsetting_cmd_result {
        Some(s) => parse_gsettings_font(s).to_string(),
        None => fallback_font_name.to_string(),
    }
}

// 'Ubuntu Mono 13' => Ubuntu Mono
#[cfg(target_os = "linux")]
fn parse_gsettings_font(input: &str) -> &str {
    use std::char;
    let input = input.trim();
    let input = input.trim_matches('\'');
    let input = input.trim_end_matches(char::is_numeric);
    let input = input.trim();
    input
}

#[test]
#[cfg(target_os = "linux")]
fn test_parse_gsettings_font() {
    assert_eq!(parse_gsettings_font("'Ubuntu 11'"), "Ubuntu");
    assert_eq!(parse_gsettings_font("'Ubuntu Mono 13'"), "Ubuntu Mono");
}

// The next three functions are taken from:
// https://github.com/christolliday/limn/blob/master/core/src/resources/image.rs

#[cfg(feature = "image_loading")]
fn prepare_image(image_decoded: DynamicImage)
    -> Result<(ImageData, ImageDescriptor), ImageError>
{
    use image;

    let image_dims = image_decoded.dimensions();

    // see: https://github.com/servo/webrender/blob/80c614ab660bf6cca52594d0e33a0be262a7ac12/wrench/src/yaml_frame_reader.rs#L401-L427
    let (format, bytes) = match image_decoded {
        image::ImageLuma8(bytes) => {
            let mut pixels = Vec::with_capacity(image_dims.0 as usize * image_dims.1 as usize * 4);
            for grey in bytes.into_iter() {
                pixels.extend_from_slice(&[
                    *grey,
                    *grey,
                    *grey,
                    0xff,
                ]);
            }
            (RawImageFormat::BGRA8, pixels)
        },
        image::ImageLumaA8(bytes) => {
            let mut pixels = Vec::with_capacity(image_dims.0 as usize * image_dims.1 as usize * 4);
            for greyscale_alpha in bytes.chunks(2) {
                let grey = greyscale_alpha[0];
                let alpha = greyscale_alpha[1];
                pixels.extend_from_slice(&[
                    grey,
                    grey,
                    grey,
                    alpha,
                ]);
            }
            (RawImageFormat::BGRA8, pixels)
        },
        image::ImageRgba8(bytes) => {
            let mut pixels = bytes.into_raw();
            // no extra allocation necessary, but swizzling
            for rgba in pixels.chunks_mut(4) {
                let r = rgba[0];
                let g = rgba[1];
                let b = rgba[2];
                let a = rgba[3];
                rgba[0] = b;
                rgba[1] = r;
                rgba[2] = g;
                rgba[3] = a;
            }
            premultiply(pixels.as_mut_slice());
            (RawImageFormat::BGRA8, pixels)
        },
        image::ImageRgb8(bytes) => {
            let mut pixels = Vec::with_capacity(image_dims.0 as usize * image_dims.1 as usize * 4);
            for rgb in bytes.chunks(3) {
                pixels.extend_from_slice(&[
                    rgb[2], // b
                    rgb[1], // g
                    rgb[0], // r
                    0xff    // a
                ]);
            }
            (RawImageFormat::BGRA8, pixels)
        },
        image::ImageBgr8(bytes) => {
            let mut pixels = Vec::with_capacity(image_dims.0 as usize * image_dims.1 as usize * 4);
            for bgr in bytes.chunks(3) {
                pixels.extend_from_slice(&[
                    bgr[0], // b
                    bgr[1], // g
                    bgr[2], // r
                    0xff    // a
                ]);
            }
            (RawImageFormat::BGRA8, pixels)
        },
        image::ImageBgra8(bytes) => {
            // Already in the correct format
            let mut pixels = bytes.into_raw();
            premultiply(pixels.as_mut_slice());
            (RawImageFormat::BGRA8, pixels)
        },
    };

    let is_opaque = is_image_opaque(format, &bytes[..]);
    let allow_mipmaps = true;
    let descriptor = ImageDescriptor::new(
        image_dims.0 as i32,
        image_dims.1 as i32,
        format,
        is_opaque,
        allow_mipmaps
    );
    let data = ImageData::new(bytes);

    Ok((data, descriptor))
}

/*
#[test]
fn test_font_gc() {

    use std::collections::BTreeMap;
    use prelude::*;
    use ui_description::UiDescription;
    use ui_state::{UiState, ui_state_from_dom};
    use ui_solver::px_to_au;
    use crate::{FastHashMap, FastHashSet};
    use std::hash::Hash;

    struct Mock;

    let mut app_resources = AppResources::new();
    let mut focused_node = None;
    let mut pending_focus_target = None;
    let is_mouse_down = false;
    let hovered_nodes = BTreeMap::new();
    let css = css::from_str(r#"
        #one { font-family: Helvetica; }
        #two { font-family: Arial; }
        #three { font-family: Times New Roman; }
    "#).unwrap();

    let mut ui_state_frame_1: UiState<Mock> = ui_state_from_dom(DomXml::mock(r#"
        <p id="one">Hello</p>
        <p id="two">Hello</p>
        <p id="three">Hello</p>
    "#).into_dom());
    let ui_description_frame_1 = UiDescription::match_css_to_dom(&mut ui_state_frame_1, &css, &mut focused_node, &mut pending_focus_target, &hovered_nodes, is_mouse_down);
    let display_list_frame_1 = DisplayList::new_from_ui_description(&ui_description_frame_1, &ui_state_frame_1);


    let mut ui_state_frame_2: UiState<Mock> = ui_state_from_dom(DomXml::mock(r#"
        <p>Hello</p>
    "#).into_dom());
    let ui_description_frame_2 = UiDescription::match_css_to_dom(&mut ui_state_frame_2, &css, &mut focused_node, &mut pending_focus_target, &hovered_nodes, is_mouse_down);
    let display_list_frame_2 = DisplayList::new_from_ui_description(&ui_description_frame_2, &ui_state_frame_2);


    let mut ui_state_frame_3: UiState<Mock> = ui_state_from_dom(DomXml::mock(r#"
        <p id="one">Hello</p>
        <p id="two">Hello</p>
        <p id="three">Hello</p>
    "#).into_dom());
    let ui_description_frame_3 = UiDescription::match_css_to_dom(&mut ui_state_frame_3, &css, &mut focused_node, &mut pending_focus_target, &hovered_nodes, is_mouse_down);
    let display_list_frame_3 = DisplayList {
        ui_descr: &ui_description_frame_3,
        rectangles: &ui_state_frame_3
    };


    // Assert that all fonts got added and detected correctly
    let mut expected_fonts = FastHashMap::new();
    expected_fonts.insert(FontId::new(), FontSource::System(String::from("Helvetica")));
    expected_fonts.insert(FontId::new(), FontSource::System(String::from("Arial")));
    expected_fonts.insert(FontId::new(), FontSource::System(String::from("Times New Roman")));

    fn build_map<T: Hash + Eq, U>(i: Vec<(T, U)>) -> FastHashMap<T, U> {
        let mut map = FastHashMap::default();
        for (k, v) in i { map.insert(k, v); }
        map
    }

    fn build_set<T: Hash + Eq>(i: Vec<T>) -> FastHashSet<T> {
        let mut set = FastHashSet::default();
        for x in i { set.insert(x); }
        set
    }

    assert_eq!(scan_ui_description_for_image_keys(&app_resources, &display_list_frame_1), FastHashSet::default());
    assert_eq!(scan_ui_description_for_image_keys(&app_resources, &display_list_frame_2), FastHashSet::default());
    assert_eq!(scan_ui_description_for_image_keys(&app_resources, &display_list_frame_3), FastHashSet::default());

    assert_eq!(scan_ui_description_for_font_keys(&app_resources, &display_list_frame_1), build_map(vec![
        (ImmediateFontId::Unresolved("Arial".to_string()), build_set(vec![px_to_au(10.0)])),
        (ImmediateFontId::Unresolved("Helvetica".to_string()), build_set(vec![px_to_au(10.0)])),
        (ImmediateFontId::Unresolved("Times New Roman".to_string()), build_set(vec![px_to_au(10.0)])),
    ]));
    assert_eq!(scan_ui_description_for_font_keys(&app_resources, &display_list_frame_2), build_map(vec![
        (ImmediateFontId::Unresolved("sans-serif".to_string()), build_set(vec![px_to_au(10.0)])),
    ]));
    assert_eq!(scan_ui_description_for_font_keys(&app_resources, &display_list_frame_3), build_map(vec![
        (ImmediateFontId::Unresolved("Arial".to_string()), build_set(vec![px_to_au(10.0)])),
        (ImmediateFontId::Unresolved("Helvetica".to_string()), build_set(vec![px_to_au(10.0)])),
        (ImmediateFontId::Unresolved("Times New Roman".to_string()), build_set(vec![px_to_au(10.0)])),
    ]));

    let mut fake_render_api = FakeRenderApi::new();

    add_fonts_and_images(&mut app_resources, &mut fake_render_api, &display_list_frame_1);
    assert_eq!(app_resources.currently_registered_fonts.len(), 3);
    assert_eq!(app_resources.last_frame_font_keys.len(), 3);

    // Assert that the first frame doesn't delete the fonts again
    garbage_collect_fonts_and_images(&mut app_resources, &mut fake_render_api);
    assert_eq!(app_resources.currently_registered_fonts.len(), 3); // fails

    // Assert that fonts don't get double-inserted, still the same font sources as previously
    add_fonts_and_images(&mut app_resources, &mut fake_render_api, &display_list_frame_3);
    garbage_collect_fonts_and_images(&mut app_resources, &mut fake_render_api);
    assert_eq!(app_resources.currently_registered_fonts.len(), 3);

    // Assert that no new fonts get added on subsequent frames
    add_fonts_and_images(&mut app_resources, &mut fake_render_api, &display_list_frame_3);
    add_fonts_and_images(&mut app_resources, &mut fake_render_api, &display_list_frame_3);
    add_fonts_and_images(&mut app_resources, &mut fake_render_api, &display_list_frame_3);
    add_fonts_and_images(&mut app_resources, &mut fake_render_api, &display_list_frame_3);
    add_fonts_and_images(&mut app_resources, &mut fake_render_api, &display_list_frame_3);
    garbage_collect_fonts_and_images(&mut app_resources, &mut fake_render_api);
    assert_eq!(app_resources.currently_registered_fonts.len(), 3);

    // If the DOM changes, the fonts should get deleted, the only font still present is "sans-serif"
    add_fonts_and_images(&mut app_resources, &mut fake_render_api, &display_list_frame_2);
    garbage_collect_fonts_and_images(&mut app_resources, &mut fake_render_api);
    assert_eq!(app_resources.currently_registered_fonts.len(), 1);

    add_fonts_and_images(&mut app_resources, &mut fake_render_api, &display_list_frame_1);
    garbage_collect_fonts_and_images(&mut app_resources, &mut fake_render_api);
    assert_eq!(app_resources.currently_registered_fonts.len(), 3);
}*/