use std::{
    collections::BTreeMap,
    rc::Rc,
};
use gleam::gl::Gl;
use webrender::api::{
    Epoch, ImageData, AddImage, ExternalImageData,
    ExternalImageType, TextureTarget, ExternalImageId,
};
use azul_core::{
    callbacks::{PipelineId, DefaultCallbackIdMap},
    app_resources::{ImageId, FontInstanceKey, ImageKey, ImageDescriptor},
    ui_solver::{
        PositionedRectangle, ResolvedOffsets, ExternalScrollId,
        LayoutResult, ScrolledNodes, OverflowingScrollNode
    },
    gl::Texture,
    display_list::{
        CachedDisplayList, DisplayListMsg, LayoutRectContent,
        ImageRendering, AlphaType, DisplayListFrame, StyleBoxShadow, DisplayListScrollFrame,
        StyleBorderStyles, StyleBorderColors, StyleBorderRadius, StyleBorderWidths,
    },
    window::FullWindowState,
};
use azul_css::{
    Css, LayoutPosition, CssProperty, ColorU, BoxShadowClipMode,
    RectStyle, RectLayout, CssPropertyValue, LayoutPoint, LayoutSize, LayoutRect,
};
use azul_layout::{GetStyle, style::Style};
use crate::{
    FastHashMap,
    app_resources::{AppResources, AddImageMsg, FontImageApi},
    callbacks::{IFrameCallback, GlCallback, StackCheckedPointer},
    ui_state::UiState,
    ui_description::{UiDescription, StyledNode},
    id_tree::{NodeDataContainer, NodeId, NodeHierarchy},
    dom::{
        DomId, NodeData, ScrollTagId, DomString,
        NodeType::{self, Div, Text, Image, GlTexture, IFrame, Label},
    },
    ui_solver::do_the_layout,
    window::{Window, FakeDisplay, WindowSize},
    callbacks::LayoutInfo,
    text_layout::LayoutedGlyphs,
};

pub(crate) struct DisplayList {
    pub(crate) rectangles: NodeDataContainer<DisplayRectangle>
}

/// Since the display list can take a lot of parameters, we don't want to
/// continually pass them as parameters of the function and rather use a
/// struct to pass them around. This is purely for ergonomic reasons.
///
/// `DisplayListParametersRef` has only members that are
///  **immutable references** to other things that need to be passed down the display list
#[derive(Clone)]
struct DisplayListParametersRef<'a, T: 'a> {
    /// ID of this Dom
    pub dom_id: DomId,
    /// The CSS that should be applied to the DOM
    pub full_window_state: &'a FullWindowState,
    /// The current pipeline of the display list
    pub pipeline_id: PipelineId,
    /// Cached layouts (+ solved layouts for iframes)
    pub layout_result: &'a SolvedLayoutCache,
    /// Cached rendered OpenGL textures
    pub gl_texture_cache: &'a GlTextureCache,
    /// Reference to the UIState, for access to `node_hierarchy` and `node_data`
    pub ui_state_cache: &'a BTreeMap<DomId, UiState<T>>,
    /// Reference to the display list (with cascaded styles)
    pub display_list_cache: &'a BTreeMap<DomId, DisplayList>,
    /// Stores the rects in rendering order
    pub rects_in_rendering_order: &'a BTreeMap<DomId, ContentGroup>,
}

/// DisplayRectangle is the main type which the layout parsing step gets operated on.
#[derive(Debug)]
pub(crate) struct DisplayRectangle {
    /// `Some(id)` if this rectangle has a callback attached to it
    /// Note: this is not the same as the `NodeId`!
    /// These two are completely separate numbers!
    pub tag: Option<u64>,
    /// The style properties of the node, parsed
    pub(crate) style: RectStyle,
    /// The layout properties of the node, parsed
    pub(crate) layout: RectLayout,
}

impl DisplayRectangle {
    #[inline]
    pub fn new(tag: Option<u64>) -> Self {
        Self {
            tag,
            style: RectStyle::default(),
            layout: RectLayout::default(),
        }
    }
}

impl GetStyle for DisplayRectangle {

    fn get_style(&self) -> Style {

        use azul_layout::{style::*, Size, Offsets, Number};
        use azul_css::{
            PixelValue, LayoutDisplay, LayoutDirection, LayoutWrap,
            LayoutAlignItems, LayoutAlignContent, LayoutJustifyContent,
            LayoutBoxSizing, Overflow as LayoutOverflow,
        };
        use azul_core::ui_solver::DEFAULT_FONT_SIZE;

        let rect_layout = &self.layout;
        let rect_style = &self.style;

        #[inline]
        fn translate_dimension(input: Option<CssPropertyValue<PixelValue>>) -> Dimension {
            use azul_css::{SizeMetric, EM_HEIGHT, PT_TO_PX};
            match input {
                None => Dimension::Undefined,
                Some(CssPropertyValue::Auto) => Dimension::Auto,
                Some(CssPropertyValue::None) => Dimension::Pixels(0.0),
                Some(CssPropertyValue::Initial) => Dimension::Undefined,
                Some(CssPropertyValue::Inherit) => Dimension::Undefined,
                Some(CssPropertyValue::Exact(pixel_value)) => match pixel_value.metric {
                    SizeMetric::Px => Dimension::Pixels(pixel_value.number.get()),
                    SizeMetric::Percent => Dimension::Percent(pixel_value.number.get()),
                    SizeMetric::Pt => Dimension::Pixels(pixel_value.number.get() * PT_TO_PX),
                    SizeMetric::Em => Dimension::Pixels(pixel_value.number.get() * EM_HEIGHT),
                }
            }
        }

        Style {
            display: match rect_layout.display {
                None => Display::Flex,
                Some(CssPropertyValue::Auto) => Display::Flex,
                Some(CssPropertyValue::None) => Display::None,
                Some(CssPropertyValue::Initial) => Display::Flex,
                Some(CssPropertyValue::Inherit) => Display::Flex,
                Some(CssPropertyValue::Exact(LayoutDisplay::Flex)) => Display::Flex,
                Some(CssPropertyValue::Exact(LayoutDisplay::Inline)) => Display::Inline,
            },
            box_sizing: match rect_layout.box_sizing.unwrap_or_default().get_property_or_default() {
                None => BoxSizing::ContentBox,
                Some(LayoutBoxSizing::ContentBox) => BoxSizing::ContentBox,
                Some(LayoutBoxSizing::BorderBox) => BoxSizing::BorderBox,
            },
            position_type: match rect_layout.position.unwrap_or_default().get_property_or_default() {
                Some(LayoutPosition::Static) => PositionType::Relative, // todo - static?
                Some(LayoutPosition::Relative) => PositionType::Relative,
                Some(LayoutPosition::Absolute) => PositionType::Absolute,
                None => PositionType::Relative,
            },
            direction: Direction::LTR,
            flex_direction: match rect_layout.direction.unwrap_or_default().get_property_or_default() {
                Some(LayoutDirection::Row) => FlexDirection::Row,
                Some(LayoutDirection::RowReverse) => FlexDirection::RowReverse,
                Some(LayoutDirection::Column) => FlexDirection::Column,
                Some(LayoutDirection::ColumnReverse) => FlexDirection::ColumnReverse,
                None => FlexDirection::Row,
            },
            flex_wrap: match rect_layout.wrap.unwrap_or_default().get_property_or_default() {
                Some(LayoutWrap::Wrap) => FlexWrap::Wrap,
                Some(LayoutWrap::NoWrap) => FlexWrap::NoWrap,
                None => FlexWrap::Wrap,
            },
            overflow: match rect_layout.overflow_x.unwrap_or_default().get_property_or_default() {
                Some(LayoutOverflow::Scroll) => Overflow::Scroll,
                Some(LayoutOverflow::Auto) => Overflow::Scroll,
                Some(LayoutOverflow::Hidden) => Overflow::Hidden,
                Some(LayoutOverflow::Visible) => Overflow::Visible,
                None => Overflow::Scroll,
            },
            align_items: match rect_layout.align_items.unwrap_or_default().get_property_or_default() {
                Some(LayoutAlignItems::Stretch) => AlignItems::Stretch,
                Some(LayoutAlignItems::Center) => AlignItems::Center,
                Some(LayoutAlignItems::Start) => AlignItems::FlexStart,
                Some(LayoutAlignItems::End) => AlignItems::FlexEnd,
                None => AlignItems::FlexStart,
            },
            align_content: match rect_layout.align_content.unwrap_or_default().get_property_or_default() {
                Some(LayoutAlignContent::Stretch) => AlignContent::Stretch,
                Some(LayoutAlignContent::Center) => AlignContent::Center,
                Some(LayoutAlignContent::Start) => AlignContent::FlexStart,
                Some(LayoutAlignContent::End) => AlignContent::FlexEnd,
                Some(LayoutAlignContent::SpaceBetween) => AlignContent::SpaceBetween,
                Some(LayoutAlignContent::SpaceAround) => AlignContent::SpaceAround,
                None => AlignContent::Stretch,
            },
            justify_content: match rect_layout.justify_content.unwrap_or_default().get_property_or_default() {
                Some(LayoutJustifyContent::Center) => JustifyContent::Center,
                Some(LayoutJustifyContent::Start) => JustifyContent::FlexStart,
                Some(LayoutJustifyContent::End) => JustifyContent::FlexEnd,
                Some(LayoutJustifyContent::SpaceBetween) => JustifyContent::SpaceBetween,
                Some(LayoutJustifyContent::SpaceAround) => JustifyContent::SpaceAround,
                Some(LayoutJustifyContent::SpaceEvenly) => JustifyContent::SpaceEvenly,
                None => JustifyContent::FlexStart,
            },
            position: Offsets {
                left: translate_dimension(rect_layout.left.map(|prop| prop.map_property(|l| l.0))),
                right: translate_dimension(rect_layout.right.map(|prop| prop.map_property(|r| r.0))),
                top: translate_dimension(rect_layout.top.map(|prop| prop.map_property(|t| t.0))),
                bottom: translate_dimension(rect_layout.bottom.map(|prop| prop.map_property(|b| b.0))),
            },
            margin: Offsets {
                left: translate_dimension(rect_layout.margin_left.map(|prop| prop.map_property(|l| l.0))),
                right: translate_dimension(rect_layout.margin_right.map(|prop| prop.map_property(|r| r.0))),
                top: translate_dimension(rect_layout.margin_top.map(|prop| prop.map_property(|t| t.0))),
                bottom: translate_dimension(rect_layout.margin_bottom.map(|prop| prop.map_property(|b| b.0))),
            },
            padding: Offsets {
                left: translate_dimension(rect_layout.padding_left.map(|prop| prop.map_property(|l| l.0))),
                right: translate_dimension(rect_layout.padding_right.map(|prop| prop.map_property(|r| r.0))),
                top: translate_dimension(rect_layout.padding_top.map(|prop| prop.map_property(|t| t.0))),
                bottom: translate_dimension(rect_layout.padding_bottom.map(|prop| prop.map_property(|b| b.0))),
            },
            border: Offsets {
                left: translate_dimension(rect_layout.border_left_width.map(|prop| prop.map_property(|l| l.0))),
                right: translate_dimension(rect_layout.border_right_width.map(|prop| prop.map_property(|r| r.0))),
                top: translate_dimension(rect_layout.border_top_width.map(|prop| prop.map_property(|t| t.0))),
                bottom: translate_dimension(rect_layout.border_bottom_width.map(|prop| prop.map_property(|b| b.0))),
            },
            flex_grow: rect_layout.flex_grow.unwrap_or_default().get_property_or_default().unwrap_or_default().0.get(),
            flex_shrink: rect_layout.flex_shrink.unwrap_or_default().get_property_or_default().unwrap_or_default().0.get(),
            size: Size {
                width: translate_dimension(rect_layout.width.map(|prop| prop.map_property(|l| l.0))),
                height: translate_dimension(rect_layout.height.map(|prop| prop.map_property(|l| l.0))),
            },
            min_size: Size {
                width: translate_dimension(rect_layout.min_width.map(|prop| prop.map_property(|l| l.0))),
                height: translate_dimension(rect_layout.min_height.map(|prop| prop.map_property(|l| l.0))),
            },
            max_size: Size {
                width: translate_dimension(rect_layout.max_width.map(|prop| prop.map_property(|l| l.0))),
                height: translate_dimension(rect_layout.max_height.map(|prop| prop.map_property(|l| l.0))),
            },
            align_self: AlignSelf::Auto, // todo!
            flex_basis: Dimension::Auto, // todo!
            aspect_ratio: Number::Undefined,
            font_size_px: rect_style.font_size.and_then(|fs| fs.get_property_owned()).unwrap_or(DEFAULT_FONT_SIZE).0,
            line_height: rect_style.line_height.and_then(|lh| lh.map_property(|lh| lh.0).get_property_owned()).map(|lh| lh.get()),
            letter_spacing: rect_style.letter_spacing.and_then(|ls| ls.map_property(|ls| ls.0).get_property_owned()),
            word_spacing: rect_style.word_spacing.and_then(|ws| ws.map_property(|ws| ws.0).get_property_owned()),
            tab_width: rect_style.tab_width.and_then(|tw| tw.map_property(|tw| tw.0).get_property_owned()).map(|tw| tw.get()),
        }
    }
}

/// Parameters that apply to a single rectangle / div node
#[derive(Copy, Clone)]
struct LayoutRectParams<'a, T: 'a> {
    epoch: Epoch,
    rect_idx: NodeId,
    html_node: &'a NodeType<T>,
    window_size: WindowSize,
}

#[derive(Debug, Clone, PartialEq)]
struct ContentGroup {
    /// The parent of the current node group, i.e. either the root node (0)
    /// or the last positioned node ()
    root: NodeId,
    /// Node ids in order of drawing
    children: Vec<ContentGroup>,
}

pub(crate) struct SolvedLayoutCache {
    pub(crate) solved_layouts: BTreeMap<DomId, LayoutResult>,
    pub(crate) display_lists: BTreeMap<DomId, DisplayList>,
    pub(crate) iframe_mappings: BTreeMap<(DomId, NodeId), DomId>,
    pub(crate) scrollable_nodes: BTreeMap<DomId, ScrolledNodes>,
    pub(crate) rects_in_rendering_order: BTreeMap<DomId, ContentGroup>,
}

pub(crate) struct GlTextureCache {
    pub(crate) solved_textures: BTreeMap<DomId, BTreeMap<NodeId, (ImageKey, ImageDescriptor, ExternalImageId)>>,
}

/// Does the layout, updates the image + font resources for the RenderAPI
pub(crate) fn do_layout_for_display_list<T>(
    data: &mut T,
    app_resources: &mut AppResources,
    window: &mut Window<T>,
    fake_display: &mut FakeDisplay<T>,
    ui_states: &mut BTreeMap<DomId, UiState<T>>,
    ui_descriptions: &mut BTreeMap<DomId, UiDescription<T>>,
    full_window_state: &mut FullWindowState,
    default_callbacks: &mut BTreeMap<DomId, DefaultCallbackIdMap<T>>,
) -> (SolvedLayoutCache, GlTextureCache) {

    use azul_css::LayoutRect;
    use crate::wr_translate::translate_logical_size_to_css_layout_size;
    use crate::app_resources::{FontImageApi, add_resources, garbage_collect_fonts_and_images};

    let pipeline_id = window.internal.pipeline_id;

    let mut layout_cache = SolvedLayoutCache {
        solved_layouts: BTreeMap::new(),
        display_lists: BTreeMap::new(),
        iframe_mappings: BTreeMap::new(),
    };

    let mut solved_textures = BTreeMap::new();
    let mut iframe_ui_states = BTreeMap::new();
    let mut iframe_ui_descriptions = BTreeMap::new();

    fn recurse<T, U: FontImageApi>(
        data: &mut T,
        layout_cache: &mut SolvedLayoutCache,
        solved_textures: &mut BTreeMap<DomId, BTreeMap<NodeId, Texture>>,
        iframe_ui_states: &mut BTreeMap<DomId, UiState<T>>,
        iframe_ui_descriptions: &mut BTreeMap<DomId, UiDescription<T>>,
        default_callbacks: &mut BTreeMap<DomId, DefaultCallbackIdMap<T>>,
        app_resources: &mut AppResources,
        render_api: &mut U,
        full_window_state: &mut FullWindowState,
        ui_state: &UiState<T>,
        ui_description: &UiDescription<T>,
        pipeline_id: &PipelineId,
        bounds: LayoutRect,
        gl_context: Rc<Gl>,
    ) {
        use azul_core::{
            callbacks::{LayoutInfo, IFrameCallbackInfoUnchecked, GlCallbackInfoUnchecked},
            ui_state::{
                ui_state_from_dom,
                scan_ui_state_for_iframe_callbacks,
                scan_ui_state_for_gltexture_callbacks,
            }
        };
        use crate::{
            display_list::{
                determine_rendering_order,
                display_list_from_ui_description,
                get_nodes_that_need_scroll_clip,
            },
            ui_solver::do_the_layout,
            wr_translate::hidpi_rect_from_bounds,
            app_resources::add_fonts_and_images,
        };
        use gleam::gl;

        // Right now the IFrameCallbacks and GlTextureCallbacks need to know how large their
        // containers are in order to be solved properly
        let iframe_callbacks = scan_ui_state_for_iframe_callbacks(ui_state);
        let gltexture_callbacks = scan_ui_state_for_gltexture_callbacks(ui_state);
        let display_list = display_list_from_ui_description(ui_description, ui_state);
        let dom_id = ui_state.dom_id.clone();

        let rects_in_rendering_order = determine_rendering_order(
            &ui_description.ui_descr_arena.node_hierarchy,
            &display_list.rectangles
        );

        // In order to calculate the layout, font + image metrics have to be calculated first
        add_fonts_and_images(app_resources, render_api, &pipeline_id, &display_list, &ui_description.ui_descr_arena.node_data);

        let layout_result = do_the_layout(
            &display_list.ui_descr.ui_descr_arena.node_layout,
            &display_list.ui_descr.ui_descr_arena.node_data,
            &display_list.rectangles,
            &app_resources,
            pipeline_id,
            bounds,
        );

        let scrollable_nodes = get_nodes_that_need_scroll_clip(
            &ui_description.ui_descr_arena.node_hierarchy,
            &display_list.rectangles,
            &ui_description.ui_descr_arena.node_data,
            &layout_result.rects,
            &layout_result.node_depths,
            *pipeline_id,
        );

        // Now the size of rects are known, render all the OpenGL textures
        for (node_id, cb, ptr) in gltexture_callbacks {

            // Invoke OpenGL callback, render texture
            let rect_bounds = layout_result.rects[node_id].bounds;

            // TODO: Unused!
            let mut window_size_width_stops = Vec::new();
            let mut window_size_height_stops = Vec::new();

            let texture = {

                let tex = (cb.0)(GlCallbackInfoUnchecked {
                    ptr,
                    layout_info: LayoutInfo {
                        window_size: &full_window_state.size,
                        window_size_width_stops: &mut window_size_width_stops,
                        window_size_height_stops: &mut window_size_height_stops,
                        default_callbacks: default_callbacks.entry(dom_id.clone()).or_insert_with(|| BTreeMap::new()),
                        gl_context: gl_context.clone(),
                        resources: &app_resources,
                    },
                    bounds: hidpi_rect_from_bounds(
                        rect_bounds,
                        full_window_state.size.hidpi_factor,
                        full_window_state.size.winit_hidpi_factor,
                    ),
                });

                // Reset the framebuffer and SRGB color target to 0
                gl_context.bind_framebuffer(gl::FRAMEBUFFER, 0);
                gl_context.disable(gl::FRAMEBUFFER_SRGB);
                gl_context.disable(gl::MULTISAMPLE);

                tex
            };

            if let Some(t) = texture {
                solved_textures
                    .entry(dom_id)
                    .or_insert_with(|| BTreeMap::default())
                    .insert(node_id, t);
            }
        }

        // Call IFrames and recurse
        for (node_id, cb, ptr) in iframe_callbacks {

            let bounds = layout_result.rects[node_id].bounds;
            let hidpi_bounds = hidpi_rect_from_bounds(
                bounds,
                full_window_state.size.hidpi_factor,
                full_window_state.size.winit_hidpi_factor
            );

            // TODO: Unused!
            let mut window_size_width_stops = Vec::new();
            let mut window_size_height_stops = Vec::new();

            let iframe_dom = {
                (cb.0)(IFrameCallbackInfoUnchecked {
                    ptr,
                    layout_info: LayoutInfo {
                        window_size: &full_window_state.size,
                        window_size_width_stops: &mut window_size_width_stops,
                        window_size_height_stops: &mut window_size_height_stops,
                        default_callbacks: default_callbacks.entry(dom_id.clone()).or_insert_with(|| BTreeMap::new()),
                        gl_context: gl_context.clone(),
                        resources: &app_resources,
                    },
                    bounds: hidpi_bounds,
                })
            };

            if let Some(iframe_dom) = iframe_dom {
                let is_mouse_down = full_window_state.mouse_state.mouse_down();
                let mut iframe_ui_state = ui_state_from_dom(iframe_dom, Some((dom_id.clone(), node_id)));
                let iframe_dom_id = iframe_ui_state.dom_id.clone();
                let hovered_nodes = full_window_state.hovered_nodes.get(&iframe_dom_id).cloned().unwrap_or_default();
                let iframe_ui_description = UiDescription::<T>::match_css_to_dom(
                    &mut iframe_ui_state,
                    &full_window_state.css,
                    &mut full_window_state.focus_target,
                    &mut full_window_state.pending_focus_target,
                    &hovered_nodes,
                    is_mouse_down,
                );
                layout_cache.iframe_mappings.insert((dom_id, node_id), iframe_dom_id);
                recurse(
                    data,
                    layout_cache,
                    solved_textures,
                    iframe_ui_states,
                    iframe_ui_descriptions,
                    default_callbacks,
                    app_resources,
                    render_api,
                    full_window_state,
                    &iframe_ui_state,
                    &iframe_ui_description,
                    pipeline_id,
                    bounds,
                    gl_context.clone(),
                );
                iframe_ui_states.insert(iframe_dom_id, iframe_ui_state);
                iframe_ui_descriptions.insert(iframe_dom_id, iframe_ui_description);
            }
        }

        layout_cache.solved_layouts.insert(dom_id, layout_result);
        layout_cache.display_lists.insert(dom_id, display_list);
        layout_cache.rects_in_rendering_order.insert(dom_id, rects_in_rendering_order);
        layout_cache.scrollable_nodes.insert(dom_id, scrollable_nodes);
    }

    // Make sure unused scroll states are garbage collected.
    window.internal.scroll_states.remove_unused_scroll_states();

    fake_display.hidden_context.make_not_current();
    window.display.make_current();

    for (dom_id, ui_state) in ui_states {

        let ui_description = &ui_descriptions[dom_id];

        DomId::reset();

        let gl_context = fake_display.get_gl_context();
        recurse(
            data,
            &mut layout_cache,
            &mut solved_textures,
            &mut iframe_ui_states,
            &mut iframe_ui_descriptions,
            default_callbacks,
            app_resources,
            &mut fake_display.render_api,
            full_window_state,
            ui_state,
            ui_description,
            &pipeline_id,
            LayoutRect {
                origin: LayoutPoint::new(0.0, 0.0),
                size: translate_logical_size_to_css_layout_size(full_window_state.size.dimensions),
            },
            gl_context,
        );
    }

    window.display.make_not_current();
    fake_display.hidden_context.make_current();

    ui_states.extend(iframe_ui_states.into_iter());
    ui_descriptions.extend(iframe_ui_descriptions.into_iter());

    let mut texture_cache = GlTextureCache {
        solved_textures: BTreeMap::new(),
    };

    let mut image_resource_updates = BTreeMap::new()
    for (dom_id, textures) in solved_textures {
        for (node_id, texture) in textures {

        const TEXTURE_IS_OPAQUE: bool = false;
        // The texture gets mapped 1:1 onto the display, so there is no need for mipmaps
        const TEXTURE_ALLOW_MIPMAPS: bool = false;

        // Note: The ImageDescriptor has no effect on how large the image appears on-screen
        let descriptor = ImageDescriptor {
            format: RawImageFormat::RGBA8,
            dimensions: (texture.size.width as usize, texture.size.height as usize),
            stride: None,
            offset: 0,
            is_opaque: TEXTURE_IS_OPAQUE,
            allow_mipmaps: TEXTURE_ALLOW_MIPMAPS,
        };

        let texture_width = texture.size.width;
        let texture_height = texture.size.height;

        let key = fake_display.render_api.new_image_key();
        let external_image_id = insert_into_active_gl_textures(pipeline_id, rectangle.epoch, texture);

        let add_img_msg = AddImageMsg(
            AddImage {
                key: wr_translate_image_key(key),
                descriptor: wr_translate_image_descriptor(descriptor),
                data: ImageData::External(ExternalImageData {
                    id: external_image_id,
                    channel_index: 0,
                    image_type: ExternalImageType::TextureHandle(TextureTarget::Default),
                }),
                tiling: None,
            },
            ImageInfo { key, descriptor }
        );

        image_resource_updates
            .entry(dom_id)
            .or_insert_with(|| Vec::new())
            .push((ImageId::new(), add_img_msg));

        gl_texture_cache.solved_textures
            .entry(dom_id)
            .or_insert_with(|| BTreeMap::new())
            .insert(node_id, (key, descriptor, external_image_id)));
        }
    }

    // Delete unused font and image keys (that were not used in this display list)
    garbage_collect_fonts_and_images(app_resources, &mut fake_display.render_api, &pipeline_id);
    // Add the new GL textures to the RenderApi
    add_resources(app_resources, &mut fake_display.render_api, &pipeline_id, Vec::new(), image_resource_updates)

    fake_display.hidden_context.make_not_current();

    (layout_cache, texture_cache)
}

fn determine_rendering_order<'a>(
    node_hierarchy: &NodeHierarchy,
    rectangles: &NodeDataContainer<DisplayRectangle>,
) -> ContentGroup {

    let children_sorted: BTreeMap<NodeId, Vec<NodeId>> = node_hierarchy
        .get_parents_sorted_by_depth()
        .into_iter()
        .map(|(_, parent_id)| (parent_id, sort_children_by_position(parent_id, node_hierarchy, rectangles)))
        .collect();

    let mut root_content_group = ContentGroup { root: NodeId::ZERO, children: Vec::new() };
    fill_content_group_children(&mut root_content_group, &children_sorted);
    root_content_group
}

fn fill_content_group_children(group: &mut ContentGroup, children_sorted: &BTreeMap<NodeId, Vec<NodeId>>) {
    if let Some(c) = children_sorted.get(&group.root) { // returns None for leaf nodes
        group.children = c
            .iter()
            .map(|child| ContentGroup { root: *child, children: Vec::new() })
            .collect();

        for c in &mut group.children {
            fill_content_group_children(c, children_sorted);
        }
    }
}

fn sort_children_by_position(
    parent: NodeId,
    node_hierarchy: &NodeHierarchy,
    rectangles: &NodeDataContainer<DisplayRectangle>
) -> Vec<NodeId> {
    use azul_css::LayoutPosition::*;

    let mut not_absolute_children = parent
        .children(node_hierarchy)
        .filter(|id| rectangles[*id].layout.position.and_then(|p| p.get_property_or_default()).unwrap_or_default() != Absolute)
        .collect::<Vec<NodeId>>();

    let mut absolute_children = parent
        .children(node_hierarchy)
        .filter(|id| rectangles[*id].layout.position.and_then(|p| p.get_property_or_default()).unwrap_or_default() == Absolute)
        .collect::<Vec<NodeId>>();

    // Append the position:absolute children after the regular children
    not_absolute_children.append(&mut absolute_children);
    not_absolute_children
}


/// Returns all node IDs where the children overflow the parent, together with the
/// `(parent_rect, child_rect)` - the child rect is the sum of the children.
///
/// TODO: The performance of this function can be theoretically improved:
///
/// - Unioning the rectangles is heavier than just looping through the children and
/// summing up their width / height / padding + margin.
/// - Scroll nodes only need to be inserted if the parent doesn't have `overflow: hidden`
/// activated
/// - Overflow for X and Y needs to be tracked seperately (for overflow-x / overflow-y separation),
/// so there we'd need to track in which direction the inner_rect is overflowing.
fn get_nodes_that_need_scroll_clip<T>(
    node_hierarchy: &NodeHierarchy,
    display_list_rects: &NodeDataContainer<DisplayRectangle>,
    dom_rects: &NodeDataContainer<NodeData<T>>,
    layouted_rects: &NodeDataContainer<PositionedRectangle>,
    parents: &[(usize, NodeId)],
    pipeline_id: PipelineId,
) -> ScrolledNodes {

    use azul_css::Overflow;

    let mut nodes = BTreeMap::new();
    let mut tags_to_node_ids = BTreeMap::new();

    for (_, parent) in parents {

        let parent_rect = &layouted_rects[*parent];

        let children_scroll_rect = match parent_rect.bounds.get_scroll_rect(parent.children(&node_hierarchy).map(|child_id| layouted_rects[child_id].bounds)) {
            None => continue,
            Some(sum) => sum,
        };

        // Check if the scroll rect overflows the parent bounds
        if contains_rect_rounded(&parent_rect.bounds, children_scroll_rect) {
            continue;
        }

        // If the overflow isn't "scroll", then there doesn't need to be a scroll frame
        if parent_rect.overflow == Overflow::Visible || parent_rect.overflow == Overflow::Hidden {
            continue;
        }

        let parent_dom_hash = dom_rects[*parent].calculate_node_data_hash();

        // Create an external scroll id. This id is required to preserve its
        // scroll state accross multiple frames.
        let parent_external_scroll_id  = ExternalScrollId(parent_dom_hash.0, pipeline_id);

        // Create a unique scroll tag for hit-testing
        let scroll_tag_id = match display_list_rects.get(*parent).and_then(|node| node.tag) {
            Some(existing_tag) => ScrollTagId(existing_tag),
            None => ScrollTagId::new(),
        };

        tags_to_node_ids.insert(scroll_tag_id, *parent);
        nodes.insert(*parent, OverflowingScrollNode {
            child_rect: children_scroll_rect,
            parent_external_scroll_id,
            parent_dom_hash,
            scroll_tag_id,
        });
    }

    ScrolledNodes { overflowing_nodes: nodes, tags_to_node_ids }
}

// Since there can be a small floating point error, round the item to the nearest pixel,
// then compare the rects
fn contains_rect_rounded(a: &LayoutRect, b: LayoutRect) -> bool {
    let a_x = a.origin.x.round() as isize;
    let a_y = a.origin.x.round() as isize;
    let a_width = a.size.width.round() as isize;
    let a_height = a.size.height.round() as isize;

    let b_x = b.origin.x.round() as isize;
    let b_y = b.origin.x.round() as isize;
    let b_width = b.size.width.round() as isize;
    let b_height = b.size.height.round() as isize;

    b_x >= a_x &&
    b_y >= a_y &&
    b_x + b_width <= a_x + a_width &&
    b_y + b_height <= a_y + a_height
}

fn node_needs_to_clip_children(layout: &RectLayout) -> bool {
    !(layout.is_horizontal_overflow_visible() || layout.is_vertical_overflow_visible())
}

/// NOTE: This function assumes that the UiDescription has an initialized arena
///
/// This only looks at the user-facing styles of the `UiDescription`, not the actual
/// layout. The layout is done only in the `into_display_list_builder` step.
pub(crate) fn display_list_from_ui_description<T>(ui_description: &UiDescription<T>, ui_state: &UiState<T>) -> DisplayList {

    let arena = &ui_description.ui_descr_arena;

    let mut override_warnings = Vec::new();

    let display_rect_arena = arena.node_data.transform(|_, node_id| {
        let style = &ui_description.styled_nodes[node_id];
        let tag = ui_state.node_ids_to_tag_ids.get(&node_id).map(|tag| *tag);
        let mut rect = DisplayRectangle::new(tag, style);
        override_warnings.append(&mut populate_css_properties(&mut rect, node_id, &ui_description.dynamic_css_overrides));
        rect
    });

    #[cfg(feature = "logging")] {
        for warning in override_warnings {
            error!(
                "Cannot override {} with {:?}",
                warning.default.get_type(), warning.overridden_property,
            )
        }
    }

    DisplayList {
        rectangles: display_rect_arena,
    }
}

pub(crate) fn push_rectangles_into_displaylist<'a, T>(
    epoch: Epoch,
    root_content_group: ContentGroup,
    referenced_content: &DisplayListParametersRef<'a, T>,
) -> DisplayListMsg {

    let rectangle = LayoutRectParams {
        epoch,
        rect_idx: root_content_group.root,
        html_node: referenced_content.node_data[root_content_group.root].get_node_type(),
        window_size,
    };

    let mut content = displaylist_handle_rect(
        &rectangle,
        referenced_content,
        referenced_mutable_content,
    );

    let children = root_content_group.children.into_iter().map(|child_content_group| {
        push_rectangles_into_displaylist(
            epoch,
            child_content_group,
            referenced_content,
        )
    }).collect();

    content.append_children(children);

    content
}

/// Push a single rectangle into the display list builder
fn displaylist_handle_rect<'a,'b, T, U: FontImageApi>(
    rectangle: &LayoutRectParams<'b, T>,
    referenced_content: &DisplayListParametersRef<'a, T>,
) -> DisplayListMsg {

    let DisplayListParametersRef { display_rectangle_arena, dom_id, pipeline_id, .. } = referenced_content;
    let LayoutRectParams { rect_idx, html_node, window_size, .. } = rectangle;

    let rect = &display_rectangle_arena[*rect_idx];
    let bounds = referenced_mutable_content.layout_result[dom_id].rects[*rect_idx].bounds;

    let display_list_rect_bounds = LayoutRect::new(
         LayoutPoint::new(bounds.origin.x, bounds.origin.y),
         LayoutSize::new(bounds.size.width, bounds.size.height),
    );

    let tag_id = rect.tag.map(|tag| (tag, 0)).or({
        referenced_mutable_content.scrollable_nodes[dom_id].overflowing_nodes
        .get(&rect_idx)
        .map(|scrolled| (scrolled.scroll_tag_id.0, 0))
    });

    let mut frame = DisplayListFrame {
        tag: tag_id,
        clip_rect: None,
        border_radius: StyleBorderRadius {
            top_left: rect.style.border_top_left_radius,
            top_right: rect.style.border_top_right_radius,
            bottom_left: rect.style.border_bottom_left_radius,
            bottom_right: rect.style.border_bottom_right_radius,
        },
        rect: display_list_rect_bounds,
        content: Vec::new(),
        children: Vec::new(),
    };

    if rect.style.has_box_shadow() {
        frame.content.push(LayoutRectContent::BoxShadow {
            shadow: StyleBoxShadow {
                left: rect.style.box_shadow_left,
                right: rect.style.box_shadow_right,
                top: rect.style.box_shadow_top,
                bottom: rect.style.box_shadow_bottom,
            },
            clip_mode: BoxShadowClipMode::Outset,
        });
    }

    // If the rect is hit-testing relevant, we need to push a rect anyway.
    // Otherwise the hit-testing gets confused
    if let Some(bg) = rect.style.background.as_ref().and_then(|br| br.get_property()) {

        use azul_css::{CssImageId, StyleBackgroundContent::*};
        use azul_core::display_list::RectBackground;

        fn get_image_info(app_resources: &AppResources, pipeline_id: &PipelineId, style_image_id: &CssImageId) -> Option<RectBackground> {
            let image_id = app_resources.get_css_image_id(&style_image_id.0)?;
            let image_info = app_resources.get_image_info(pipeline_id, image_id)?;
            Some(RectBackground::Image(*image_info))
        }

        let background_content = match bg {
            LinearGradient(lg) => Some(RectBackground::LinearGradient(lg.clone())),
            RadialGradient(rg) => Some(RectBackground::RadialGradient(rg.clone())),
            Image(style_image_id) => get_image_info(referenced_mutable_content.app_resources, &referenced_content.pipeline_id, style_image_id),
            Color(c) => Some(RectBackground::Color(*c)),
        };

        if let Some(background_content) = background_content {
            frame.content.push(LayoutRectContent::Background {
                content: background_content,
                size: rect.style.background_size.and_then(|bs| bs.get_property().cloned()),
                offset: rect.style.background_position.and_then(|bs| bs.get_property().cloned()),
                repeat: rect.style.background_repeat.and_then(|bs| bs.get_property().cloned()),
            });
        }
    }

    match html_node {
        Div => { },
        Text(_) | Label(_) => {
            if let Some(layouted_glyphs) = referenced_mutable_content.layout_result[dom_id].layouted_glyph_cache.get(&rect_idx).cloned() {

                use azul_core::ui_solver::DEFAULT_FONT_COLOR;
                use crate::wr_translate::translate_logical_size_to_css_layout_size;

                let text_color = rect.style.text_color.and_then(|tc| tc.get_property().cloned()).unwrap_or(DEFAULT_FONT_COLOR).0;
                let positioned_words = &referenced_mutable_content.layout_result[dom_id].positioned_word_cache[&rect_idx];
                let font_instance_key = positioned_words.1;

                frame.content.push(get_text(
                    display_list_rect_bounds,
                    &referenced_mutable_content.layout_result[dom_id].rects[*rect_idx].padding,
                    translate_logical_size_to_css_layout_size(window_size.dimensions),
                    layouted_glyphs,
                    font_instance_key,
                    text_color,
                    &rect.layout,
                ));
            }
        },
        Image(image_id) => {
            if let Some(image_info) = referenced_mutable_content.app_resources.get_image_info(pipeline_id, image_id) {
                frame.content.push(LayoutRectContent::Image {
                    size: LayoutSize::new(bounds.size.width, bounds.size.height),
                    offset: LayoutPoint::new(0.0, 0.0),
                    image_rendering: ImageRendering::Auto,
                    alpha_type: AlphaType::PremultipliedAlpha,
                    image_key: image_info.key,
                    background_color: ColorU::WHITE,
                });
            }
        },
        GlTexture(_) => {
            if let Some((key, descriptor, _)) = referenced_content.solved_textures.get(&(dom_id.clone(), *rect_idx)) {
                frame.content.push(LayoutRectContent::Image {
                    size: LayoutSize::new(descriptor.size.width, descriptor.size.height),
                    offset: LayoutPoint::new(0.0, 0.0),
                    image_rendering: ImageRendering::Auto,
                    alpha_type: AlphaType::Alpha,
                    image_key: key,
                    background_color: ColorU::WHITE,
                })
            }
        },
        IFrame(_) => {
            if let Some(iframe_dom_id) = referenced_content.iframe_mappings.get(&(dom_id.clone(), *rect_idx)) {
                frame.children.push(push_rectangles_into_displaylist(
                    rectangle.epoch,
                    rects_in_rendering_order.root,
                    &DisplayListParametersRef {
                        // Important: Need to update the DOM ID,
                        // otherwise this function would be endlessly recurse
                        dom_id: iframe_dom_id,
                        .. *referenced_content
                    }
                ));
            }
        },
    };

    if rect.style.has_border() {
        frame.content.push(LayoutRectContent::Border {
            widths: StyleBorderWidths {
                top: rect.layout.border_top_width,
                left: rect.layout.border_left_width,
                bottom: rect.layout.border_bottom_width,
                right: rect.layout.border_right_width,
            },
            colors: StyleBorderColors {
                top: rect.style.border_top_color,
                left: rect.style.border_left_color,
                bottom: rect.style.border_bottom_color,
                right: rect.style.border_right_color,
            },
            styles: StyleBorderStyles {
                top: rect.style.border_top_style,
                left: rect.style.border_left_style,
                bottom: rect.style.border_bottom_style,
                right: rect.style.border_right_style,
            },
        });
    }

    if rect.style.has_box_shadow() {
        frame.content.push(LayoutRectContent::BoxShadow {
            shadow: StyleBoxShadow {
                left: rect.style.box_shadow_left,
                right: rect.style.box_shadow_right,
                top: rect.style.box_shadow_top,
                bottom: rect.style.box_shadow_bottom,
            },
            clip_mode: BoxShadowClipMode::Inset,
        });
    }

    match referenced_mutable_content.scrollable_nodes[dom_id].overflowing_nodes.get(&rect_idx) {
        Some(scroll_node) => DisplayListMsg::ScrollFrame(DisplayListScrollFrame {
            content_rect: scroll_node.child_rect,
            scroll_id: scroll_node.parent_external_scroll_id,
            scroll_tag: scroll_node.scroll_tag_id,
            frame,
        }),
        None => DisplayListMsg::Frame(frame),
    }
}

fn get_text(
    bounds: LayoutRect,
    padding: &ResolvedOffsets,
    root_window_size: LayoutSize,
    layouted_glyphs: LayoutedGlyphs,
    font_instance_key: FontInstanceKey,
    font_color: ColorU,
    rect_layout: &RectLayout,
) -> LayoutRectContent {

    let overflow_horizontal_visible = rect_layout.is_horizontal_overflow_visible();
    let overflow_vertical_visible = rect_layout.is_horizontal_overflow_visible();

    let padding_clip_bounds = subtract_padding(&bounds, padding);

    // Adjust the bounds by the padding, depending on the overflow:visible parameter
    let text_clip_rect = match (overflow_horizontal_visible, overflow_vertical_visible) {
        (true, true) => None,
        (false, false) => Some(padding_clip_bounds),
        (true, false) => {
            // Horizontally visible, vertically cut
            Some(LayoutRect {
                origin: bounds.origin,
                size: LayoutSize::new(root_window_size.width, padding_clip_bounds.size.height),
            })
        },
        (false, true) => {
            // Vertically visible, horizontally cut
            Some(LayoutRect {
                origin: bounds.origin,
                size: LayoutSize::new(padding_clip_bounds.size.width, root_window_size.height),
            })
        },
    };

    LayoutRectContent::Text {
        glyphs: layouted_glyphs.glyphs,
        font_instance_key,
        color: font_color,
        glyph_options: None,
        clip: text_clip_rect,
    }
}

/// Subtracts the padding from the bounds, returning the new bounds
///
/// Warning: The resulting rectangle may have negative width or height
fn subtract_padding(bounds: &LayoutRect, padding: &ResolvedOffsets) -> LayoutRect {

    let mut new_bounds = *bounds;

    new_bounds.origin.x += padding.left;
    new_bounds.size.width -= padding.right + padding.left;
    new_bounds.origin.y += padding.top;
    new_bounds.size.height -= padding.top + padding.bottom;

    new_bounds
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct OverrideWarning {
    pub default: CssProperty,
    pub overridden_property: CssProperty,
}

/// Populate the style properties of the `DisplayRectangle`, apply static / dynamic properties
fn populate_css_properties(
    rect: &mut DisplayRectangle,
    node_id: NodeId,
    css_overrides: &BTreeMap<NodeId, FastHashMap<DomString, CssProperty>>,
) -> Vec<OverrideWarning> {

    use azul_css::CssDeclaration::*;
    use std::mem;

    let rect_style = &mut rect.style;
    let rect_layout = &mut rect.layout;
    let css_constraints = &rect.styled_node.css_constraints;

   css_constraints
    .values()
    .filter_map(|constraint| match constraint {
        Static(static_property) => {
            apply_style_property(rect_style, rect_layout, &static_property);
            None
        },
        Dynamic(dynamic_property) => {
            let overridden_property = css_overrides.get(&node_id).and_then(|overrides| overrides.get(&dynamic_property.dynamic_id.clone().into()))?;

            // Apply the property default if the discriminant of the two types matches
            if mem::discriminant(overridden_property) == mem::discriminant(&dynamic_property.default_value) {
                apply_style_property(rect_style, rect_layout, overridden_property);
                None
            } else {
                Some(OverrideWarning {
                    default: dynamic_property.default_value.clone(),
                    overridden_property: overridden_property.clone(),
                })
            }
        },
    })
    .collect()
}

fn apply_style_property(style: &mut RectStyle, layout: &mut RectLayout, property: &CssProperty) {

    use azul_css::CssProperty::*;

    match property {

        Display(d)                      => layout.display = Some(*d),
        Float(f)                        => layout.float = Some(*f),
        BoxSizing(bs)                   => layout.box_sizing = Some(*bs),

        TextColor(c)                    => style.text_color = Some(*c),
        FontSize(fs)                    => style.font_size = Some(*fs),
        FontFamily(ff)                  => style.font_family = Some(ff.clone()),
        TextAlign(ta)                   => style.text_align = Some(*ta),

        LetterSpacing(ls)               => style.letter_spacing = Some(*ls),
        LineHeight(lh)                  => style.line_height = Some(*lh),
        WordSpacing(ws)                 => style.word_spacing = Some(*ws),
        TabWidth(tw)                    => style.tab_width = Some(*tw),
        Cursor(c)                       => style.cursor = Some(*c),

        Width(w)                        => layout.width = Some(*w),
        Height(h)                       => layout.height = Some(*h),
        MinWidth(mw)                    => layout.min_width = Some(*mw),
        MinHeight(mh)                   => layout.min_height = Some(*mh),
        MaxWidth(mw)                    => layout.max_width = Some(*mw),
        MaxHeight(mh)                   => layout.max_height = Some(*mh),

        Position(p)                     => layout.position = Some(*p),
        Top(t)                          => layout.top = Some(*t),
        Bottom(b)                       => layout.bottom = Some(*b),
        Right(r)                        => layout.right = Some(*r),
        Left(l)                         => layout.left = Some(*l),

        FlexWrap(fw)                    => layout.wrap = Some(*fw),
        FlexDirection(fd)               => layout.direction = Some(*fd),
        FlexGrow(fg)                    => layout.flex_grow = Some(*fg),
        FlexShrink(fs)                  => layout.flex_shrink = Some(*fs),
        JustifyContent(jc)              => layout.justify_content = Some(*jc),
        AlignItems(ai)                  => layout.align_items = Some(*ai),
        AlignContent(ac)                => layout.align_content = Some(*ac),

        BackgroundContent(bc)           => style.background = Some(bc.clone()),
        BackgroundPosition(bp)          => style.background_position = Some(*bp),
        BackgroundSize(bs)              => style.background_size = Some(*bs),
        BackgroundRepeat(br)            => style.background_repeat = Some(*br),

        OverflowX(ox)                   => layout.overflow_x = Some(*ox),
        OverflowY(oy)                   => layout.overflow_y = Some(*oy),

        PaddingTop(pt)                  => layout.padding_top = Some(*pt),
        PaddingLeft(pl)                 => layout.padding_left = Some(*pl),
        PaddingRight(pr)                => layout.padding_right = Some(*pr),
        PaddingBottom(pb)               => layout.padding_bottom = Some(*pb),

        MarginTop(mt)                   => layout.margin_top = Some(*mt),
        MarginLeft(ml)                  => layout.margin_left = Some(*ml),
        MarginRight(mr)                 => layout.margin_right = Some(*mr),
        MarginBottom(mb)                => layout.margin_bottom = Some(*mb),

        BorderTopLeftRadius(btl)        => style.border_top_left_radius = Some(*btl),
        BorderTopRightRadius(btr)       => style.border_top_right_radius = Some(*btr),
        BorderBottomLeftRadius(bbl)     => style.border_bottom_left_radius = Some(*bbl),
        BorderBottomRightRadius(bbr)    => style.border_bottom_right_radius = Some(*bbr),

        BorderTopColor(btc)             => style.border_top_color = Some(*btc),
        BorderRightColor(brc)           => style.border_right_color = Some(*brc),
        BorderLeftColor(blc)            => style.border_left_color = Some(*blc),
        BorderBottomColor(bbc)          => style.border_bottom_color = Some(*bbc),

        BorderTopStyle(bts)             => style.border_top_style = Some(*bts),
        BorderRightStyle(brs)           => style.border_right_style = Some(*brs),
        BorderLeftStyle(bls)            => style.border_left_style = Some(*bls),
        BorderBottomStyle(bbs)          => style.border_bottom_style = Some(*bbs),

        BorderTopWidth(btw)             => layout.border_top_width = Some(*btw),
        BorderRightWidth(brw)           => layout.border_right_width = Some(*brw),
        BorderLeftWidth(blw)            => layout.border_left_width = Some(*blw),
        BorderBottomWidth(bbw)          => layout.border_bottom_width = Some(*bbw),

        BoxShadowLeft(bsl)              => style.box_shadow_left = Some(*bsl),
        BoxShadowRight(bsr)             => style.box_shadow_right = Some(*bsr),
        BoxShadowTop(bst)               => style.box_shadow_top = Some(*bst),
        BoxShadowBottom(bsb)            => style.box_shadow_bottom = Some(*bsb),
    }
}

#[test]
fn test_overflow_parsing() {
    use prelude::Overflow;

    let layout1 = RectLayout::default();

    // The default for overflowing is overflow: auto, which clips
    // children, so this should evaluate to true by default
    assert_eq!(node_needs_to_clip_children(&layout1), true);

    let layout2 = RectLayout {
        overflow_x: Some(CssPropertyValue::Exact(Overflow::Visible)),
        overflow_y: Some(CssPropertyValue::Exact(Overflow::Visible)),
        .. Default::default()
    };
    assert_eq!(node_needs_to_clip_children(&layout2), false);

    let layout3 = RectLayout {
        overflow_x: Some(CssPropertyValue::Exact(Overflow::Hidden)),
        overflow_y: Some(CssPropertyValue::Exact(Overflow::Hidden)),
        .. Default::default()
    };
    assert_eq!(node_needs_to_clip_children(&layout3), true);
}