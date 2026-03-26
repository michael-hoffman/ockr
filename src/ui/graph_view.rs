//! Graph View — force-directed backlink graph (Story 19).
//!
//! Displays all vault notes as nodes in a force-directed layout.  Directed
//! edges represent wikilinks.  The view is a full-screen overlay rendered
//! with the GPUI canvas API.
//!
//! ## Layout
//!
//! - **Canvas layer** (bottom): edges (stroked paths) + node circles (quads).
//! - **Label layer** (top): absolutely-positioned text divs for node titles.
//! - **HUD** (top): query input, escape hint.
//!
//! ## Force simulation
//!
//! Runs 300 steps synchronously on view creation, producing a stable layout.
//! Forces: Coulomb repulsion (all pairs), Hooke attraction (edges), mild
//! center gravity, velocity damping.
//!
//! ## Interaction
//!
//! | Input | Effect |
//! |-------|--------|
//! | `h/j/k/l` / arrows | Move focus to nearest node in direction |
//! | `Enter` | Open focused note in editor |
//! | `Escape` | Close graph |
//! | Typing | Filter nodes by title |
//! | Backspace | Delete last search char |
//! | Scroll wheel | Zoom in/out |
//! | Left-drag (background) | Pan the camera |
//! | Click node | Focus that node |

use std::path::PathBuf;

use gpui::{
    App, Bounds, Context, EventEmitter, FocusHandle, Focusable, KeyDownEvent, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, PathBuilder, Pixels, Render,
    ScrollWheelEvent, Window, canvas, div, point, prelude::*, px, quad, rgba, size,
};

use crate::ui::theme::ThemePalette;
use crate::vault::{BacklinkIndex, VaultFile};

// ── Data model ────────────────────────────────────────────────────────────────

struct GraphNode {
    file: VaultFile,
    /// Model-space position (force simulation output, centred on 0,0).
    x: f32,
    y: f32,
    /// Velocity (only used during simulation).
    vx: f32,
    vy: f32,
    /// Total link degree (in + out) — used for colour coding.
    degree: usize,
}

// ── Events ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum GraphViewEvent {
    Close,
    Open(PathBuf),
}

impl EventEmitter<GraphViewEvent> for GraphView {}

// ── View ──────────────────────────────────────────────────────────────────────

pub struct GraphView {
    pub focus_handle: FocusHandle,
    nodes: Vec<GraphNode>,
    /// Directed edge indices into `nodes`.
    edges: Vec<(usize, usize)>,
    /// Index of the focused node.
    focused_idx: Option<usize>,
    /// Title search filter (typed in-place, no dedicated input widget).
    query: String,
    /// Camera pan in screen pixels.
    pan_x: f32,
    pan_y: f32,
    /// Camera zoom (1.0 = 100%).
    zoom: f32,
    /// Active left-button drag: (mouse_start_x, mouse_start_y, pan_start_x, pan_start_y).
    drag: Option<(f32, f32, f32, f32)>,
    /// Canvas dimensions captured during the last render (used in mouse handlers).
    canvas_w: f32,
    canvas_h: f32,
}

impl GraphView {
    /// Build the graph from the vault's file list and backlink index.
    pub fn new(
        files: Vec<VaultFile>,
        backlinks: &BacklinkIndex,
        cx: &mut Context<Self>,
    ) -> Self {
        let n = files.len();

        // Build edge list from the incoming-link index.
        let mut degree = vec![0usize; n];
        let mut edges: Vec<(usize, usize)> = Vec::new();
        for (j, target) in files.iter().enumerate() {
            for source in backlinks.incoming_links(&target.rel_path) {
                if let Some(i) = files.iter().position(|f| f.abs_path == source.abs_path) {
                    if i != j {
                        edges.push((i, j));
                        degree[i] += 1;
                        degree[j] += 1;
                    }
                }
            }
        }
        // Deduplicate (same pair can appear from multiple scan directions).
        edges.sort_unstable();
        edges.dedup();

        // Place nodes on a circle so initial positions are spread out.
        let init_r = (50.0f32).max(n as f32 * 10.0);
        let tau = 2.0 * std::f32::consts::PI;
        let nodes: Vec<GraphNode> = files.into_iter().enumerate().map(|(i, file)| {
            let a = if n > 0 { (i as f32 / n as f32) * tau } else { 0.0 };
            GraphNode {
                x: a.cos() * init_r,
                y: a.sin() * init_r,
                vx: 0.0,
                vy: 0.0,
                degree: degree[i],
                file,
            }
        }).collect();

        let mut view = Self {
            focus_handle: cx.focus_handle(),
            nodes,
            edges,
            focused_idx: if n > 0 { Some(0) } else { None },
            query: String::new(),
            pan_x: 0.0,
            pan_y: 0.0,
            zoom: 1.0,
            drag: None,
            canvas_w: 800.0,
            canvas_h: 600.0,
        };
        view.simulate(300);
        view
    }

    // ── Force simulation ─────────────────────────────────────────────────────

    fn simulate(&mut self, steps: usize) {
        let n = self.nodes.len();
        if n < 2 { return; }

        const K_REPEL: f32 = 6_000.0;
        const K_SPRING: f32 = 0.02;
        const REST_LEN: f32 = 100.0;
        const K_GRAVITY: f32 = 0.003;
        const DAMPING: f32 = 0.88;

        for _ in 0..steps {
            let mut fx = vec![0.0f32; n];
            let mut fy = vec![0.0f32; n];

            // Coulomb repulsion (all pairs O(n²), fine for n < 2000).
            for i in 0..n {
                for j in (i + 1)..n {
                    let dx = self.nodes[j].x - self.nodes[i].x;
                    let dy = self.nodes[j].y - self.nodes[i].y;
                    let d2 = (dx * dx + dy * dy).max(1.0);
                    let d = d2.sqrt();
                    let f = K_REPEL / d2;
                    let (ux, uy) = (dx / d, dy / d);
                    fx[i] -= f * ux;
                    fy[i] -= f * uy;
                    fx[j] += f * ux;
                    fy[j] += f * uy;
                }
            }

            // Hooke spring attraction along edges.
            for &(a, b) in &self.edges {
                let dx = self.nodes[b].x - self.nodes[a].x;
                let dy = self.nodes[b].y - self.nodes[a].y;
                let d = (dx * dx + dy * dy).sqrt().max(0.1);
                let f = K_SPRING * (d - REST_LEN);
                let (ux, uy) = (dx / d, dy / d);
                fx[a] += f * ux;
                fy[a] += f * uy;
                fx[b] -= f * ux;
                fy[b] -= f * uy;
            }

            // Mild center gravity.
            for i in 0..n {
                fx[i] -= self.nodes[i].x * K_GRAVITY;
                fy[i] -= self.nodes[i].y * K_GRAVITY;
            }

            // Integrate velocities with damping.
            for i in 0..n {
                self.nodes[i].vx = (self.nodes[i].vx + fx[i]) * DAMPING;
                self.nodes[i].vy = (self.nodes[i].vy + fy[i]) * DAMPING;
                self.nodes[i].x += self.nodes[i].vx;
                self.nodes[i].y += self.nodes[i].vy;
            }
        }
    }

    // ── Utilities ────────────────────────────────────────────────────────────

    /// Indices of nodes that match the current search query.
    fn visible(&self) -> Vec<usize> {
        if self.query.is_empty() {
            (0..self.nodes.len()).collect()
        } else {
            let q = self.query.to_lowercase();
            self.nodes.iter().enumerate()
                .filter(|(_, n)| n.file.title.to_lowercase().contains(&q))
                .map(|(i, _)| i)
                .collect()
        }
    }

    /// Screen position of node `i` given the current camera state.
    fn screen_pos(&self, i: usize) -> (f32, f32) {
        let cx = self.canvas_w / 2.0 + self.pan_x;
        let cy = self.canvas_h / 2.0 + self.pan_y;
        (
            self.nodes[i].x * self.zoom + cx,
            self.nodes[i].y * self.zoom + cy,
        )
    }

    /// Return the index of the node closest to screen position `(sx, sy)`
    /// within `radius` screen pixels.
    fn node_at(&self, sx: f32, sy: f32, radius: f32) -> Option<usize> {
        let visible = self.visible();
        let r2 = radius * radius;
        visible.iter().min_by(|&&a, &&b| {
            let (ax, ay) = self.screen_pos(a);
            let (bx, by) = self.screen_pos(b);
            let da = (ax - sx).powi(2) + (ay - sy).powi(2);
            let db = (bx - sy).powi(2) + (by - sy).powi(2);
            da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
        }).and_then(|&i| {
            let (nx, ny) = self.screen_pos(i);
            let d2 = (nx - sx).powi(2) + (ny - sy).powi(2);
            if d2 <= r2 { Some(i) } else { None }
        })
    }

    /// Move focus to the nearest visible node in direction `(dir_x, dir_y)`.
    fn move_focus(&mut self, dir_x: f32, dir_y: f32) {
        let Some(cur) = self.focused_idx else {
            self.focused_idx = self.visible().first().copied();
            return;
        };
        let (cx, cy) = self.screen_pos(cur);
        let visible = self.visible();
        let best = visible.iter()
            .filter(|&&i| i != cur)
            .filter_map(|&i| {
                let (nx, ny) = self.screen_pos(i);
                let dx = nx - cx;
                let dy = ny - cy;
                let dot = dx * dir_x + dy * dir_y;
                if dot > 0.0 {
                    let dist = (dx * dx + dy * dy).sqrt().max(0.1);
                    Some((i, dot / dist))
                } else {
                    None
                }
            })
            .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(i, _)| i);
        if let Some(idx) = best {
            self.focused_idx = Some(idx);
        }
    }

    // ── Event handlers ────────────────────────────────────────────────────────

    fn handle_key(&mut self, event: &KeyDownEvent, _w: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        let k = &event.keystroke;

        match k.key.as_str() {
            "escape" => { cx.emit(GraphViewEvent::Close); return; }
            "enter" => {
                if let Some(idx) = self.focused_idx {
                    cx.emit(GraphViewEvent::Open(self.nodes[idx].file.abs_path.clone()));
                } else {
                    cx.emit(GraphViewEvent::Close);
                }
                return;
            }
            "h" | "left"  => { self.move_focus(-1.0,  0.0); cx.notify(); return; }
            "l" | "right" => { self.move_focus( 1.0,  0.0); cx.notify(); return; }
            "k" | "up"    => { self.move_focus( 0.0, -1.0); cx.notify(); return; }
            "j" | "down"  => { self.move_focus( 0.0,  1.0); cx.notify(); return; }
            "=" | "+"     => { self.zoom = (self.zoom * 1.2).min(8.0); cx.notify(); return; }
            "-"           => { self.zoom = (self.zoom / 1.2).max(0.05); cx.notify(); return; }
            "backspace"   => {
                self.query.pop();
                // Ensure focused node is still visible.
                if let Some(f) = self.focused_idx {
                    if !self.visible().contains(&f) {
                        self.focused_idx = self.visible().first().copied();
                    }
                }
                cx.notify();
                return;
            }
            _ => {}
        }

        if let Some(ch) = &k.key_char {
            if !k.modifiers.control && !k.modifiers.platform {
                self.query.push_str(ch);
                // Re-anchor focus to first visible match.
                let vis = self.visible();
                if let Some(f) = self.focused_idx {
                    if !vis.contains(&f) {
                        self.focused_idx = vis.first().copied();
                    }
                }
                cx.notify();
            }
        }
    }

    fn handle_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let sx = f32::from(event.position.x);
        let sy = f32::from(event.position.y);
        if let Some(idx) = self.node_at(sx, sy, 18.0) {
            self.focused_idx = Some(idx);
            cx.notify();
        } else {
            // Start panning.
            self.drag = Some((sx, sy, self.pan_x, self.pan_y));
        }
    }

    fn handle_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some((sx, sy, px0, py0)) = self.drag {
            let mx = f32::from(event.position.x);
            let my = f32::from(event.position.y);
            self.pan_x = px0 + (mx - sx);
            self.pan_y = py0 + (my - sy);
            cx.notify();
        }
    }

    fn handle_mouse_up(
        &mut self,
        _event: &MouseUpEvent,
        _w: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        self.drag = None;
    }

    fn handle_scroll(
        &mut self,
        event: &ScrollWheelEvent,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let delta = event.delta.pixel_delta(px(20.0));
        let dy = f32::from(delta.y);
        if dy < 0.0 {
            self.zoom = (self.zoom * 1.06).min(8.0);
        } else if dy > 0.0 {
            self.zoom = (self.zoom / 1.06).max(0.05);
        }
        cx.notify();
    }
}

impl Focusable for GraphView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

// ── Render ────────────────────────────────────────────────────────────────────

impl Render for GraphView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.global::<ThemePalette>().clone();

        // Capture canvas dimensions from GPUI viewport.
        let vp = window.viewport_size();
        self.canvas_w = f32::from(vp.width);
        self.canvas_h = f32::from(vp.height);

        let visible_set = self.visible();
        let focused_idx = self.focused_idx;

        // ── Snapshot data for canvas closure (must be 'static) ───────────────
        #[derive(Clone)]
        struct NodeSnap {
            x: f32, y: f32, degree: usize, focused: bool, visible: bool,
        }
        let node_snaps: Vec<NodeSnap> = self.nodes.iter().enumerate().map(|(i, n)| NodeSnap {
            x: n.x, y: n.y,
            degree: n.degree,
            focused: focused_idx == Some(i),
            visible: visible_set.contains(&i),
        }).collect();
        let edges_snap = self.edges.clone();
        let pan_x = self.pan_x;
        let pan_y = self.pan_y;
        let zoom = self.zoom;
        let canvas_w = self.canvas_w;
        let canvas_h = self.canvas_h;

        // Colors from theme.
        let bg_color           = rgba(((t.bg_base as u64) << 8 | 0xee) as u32);
        let edge_color         = rgba(((t.text_faint as u64) << 8 | 0xcc) as u32);
        let node_isolated      = rgba(((t.text_faint as u64) << 8 | 0xff) as u32);
        let node_normal        = rgba(((t.text_muted as u64) << 8 | 0xff) as u32);
        let node_hub           = rgba(((t.ochre as u64) << 8 | 0xff) as u32);
        let node_focused_color = rgba(((t.ochre as u64) << 8 | 0xff) as u32);

        // ── Label divs (one per visible node) ────────────────────────────────
        let show_labels = zoom >= 0.5;
        let label_color_normal  = gpui::rgb(t.text_muted);
        let label_color_focused = gpui::rgb(t.text);

        let mut labels: Vec<gpui::AnyElement> = Vec::new();
        if show_labels {
            for &i in &visible_set {
                let n = &self.nodes[i];
                let (sx, sy) = self.screen_pos(i);
                let node_r: f32 = if n.degree >= 3 { 9.0 } else { 6.0 };
                let is_focused = focused_idx == Some(i);
                let title = n.file.title.clone();
                let label_color = if is_focused { label_color_focused } else { label_color_normal };

                // Truncate long titles.
                let display = if title.len() > 22 {
                    format!("{}…", &title[..21])
                } else {
                    title
                };

                labels.push(
                    div()
                        .absolute()
                        .left(px(sx - 60.0))
                        .top(px(sy + node_r as f32 + 4.0))
                        .w(px(120.0))
                        .flex()
                        .justify_center()
                        .text_xs()
                        .font_family("Menlo")
                        .text_color(label_color)
                        .child(display)
                        .into_any_element(),
                );
            }
        }

        // ── HUD: query bar + hint ────────────────────────────────────────────
        let query_display = if self.query.is_empty() {
            format!("/ search…   {} nodes   Cmd-Shift-G: close", self.nodes.len())
        } else {
            format!("/ {}  ({} matches)", self.query, visible_set.len())
        };
        let node_count = self.nodes.len();

        // ── Outer container ──────────────────────────────────────────────────
        div()
            .absolute()
            .inset_0()
            .track_focus(&self.focus_handle)
            .bg(bg_color)
            .on_key_down(cx.listener(Self::handle_key))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(Self::handle_mouse_down),
            )
            .on_mouse_move(cx.listener(Self::handle_mouse_move))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(Self::handle_mouse_up),
            )
            .on_scroll_wheel(cx.listener(Self::handle_scroll))
            // ── Canvas (edges + node circles) ──────────────────────────────
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds: Bounds<Pixels>, _prepaint, window, _cx| {
                        let ctr_x = f32::from(bounds.size.width) / 2.0 + pan_x;
                        let ctr_y = f32::from(bounds.size.height) / 2.0 + pan_y;

                        let to_screen = |mx: f32, my: f32| -> (f32, f32) {
                            (mx * zoom + ctr_x, my * zoom + ctr_y)
                        };

                        // Draw edges.
                        for &(a, b) in &edges_snap {
                            let na = &node_snaps[a];
                            let nb = &node_snaps[b];
                            if !na.visible && !nb.visible { continue; }
                            let (x1, y1) = to_screen(na.x, na.y);
                            let (x2, y2) = to_screen(nb.x, nb.y);
                            let mut builder = PathBuilder::stroke(px(1.0));
                            builder.move_to(point(px(x1), px(y1)));
                            builder.line_to(point(px(x2), px(y2)));
                            if let Ok(path) = builder.build() {
                                window.paint_path(path, edge_color);
                            }
                        }

                        // Draw node circles.
                        const BASE_R: f32 = 6.0;
                        const HUB_R:  f32 = 9.0;
                        for (i, n) in node_snaps.iter().enumerate() {
                            if !n.visible { continue; }
                            let (sx, sy) = to_screen(n.x, n.y);
                            let r = if n.degree >= 3 { HUB_R } else { BASE_R };
                            let r = r * zoom.max(0.3);
                            let color = if n.focused {
                                node_focused_color
                            } else if n.degree == 0 {
                                node_isolated
                            } else if n.degree >= 3 {
                                node_hub
                            } else {
                                node_normal
                            };

                            // Outer glow for focused node.
                            if n.focused {
                                let gr = r + 4.0;
                                let glow_bounds = gpui::Bounds {
                                    origin: point(px(sx - gr), px(sy - gr)),
                                    size: size(px(gr * 2.0), px(gr * 2.0)),
                                };
                                window.paint_quad(quad(
                                    glow_bounds,
                                    px(gr),
                                    rgba(0xcc772233_u32),
                                    px(0.0),
                                    gpui::transparent_black(),
                                    gpui::BorderStyle::Solid,
                                ));
                            }

                            // Node circle.
                            let node_bounds = gpui::Bounds {
                                origin: point(px(sx - r), px(sy - r)),
                                size: size(px(r * 2.0), px(r * 2.0)),
                            };
                            window.paint_quad(quad(
                                node_bounds,
                                px(r),
                                color,
                                px(0.0),
                                gpui::transparent_black(),
                                gpui::BorderStyle::Solid,
                            ));
                        }

                        // Empty-vault hint.
                        if node_snaps.is_empty() {
                            let _ = node_count; // used for the HUD
                        }
                    },
                )
                .absolute()
                .inset_0(),
            )
            // ── Node labels ────────────────────────────────────────────────
            .children(labels)
            // ── HUD bar (bottom) ────────────────────────────────────────────
            .child(
                div()
                    .absolute()
                    .bottom(px(0.0))
                    .left(px(0.0))
                    .right(px(0.0))
                    .h(px(32.0))
                    .bg(gpui::rgb(t.bg_panel))
                    .border_t_1()
                    .border_color(gpui::rgb(t.border_subtle))
                    .flex()
                    .items_center()
                    .px(px(16.0))
                    .gap(px(16.0))
                    .child(
                        div()
                            .text_xs()
                            .font_family("Menlo")
                            .text_color(gpui::rgb(t.ochre))
                            .child("Graph"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .font_family("Menlo")
                            .text_color(gpui::rgb(t.text_muted))
                            .child(query_display),
                    ),
            )
    }
}
