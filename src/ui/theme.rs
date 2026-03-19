//! ockr design tokens.
//!
//! Sourced from the ockr-collection reference app. All hex values are RGB.
//!
//! Palette name: Oxide
//!   Background family: near-black volcanic darks
//!   Accent: ochre (#CC7722) — oxidized metal

// ── Backgrounds ───────────────────────────────────────────────────────────────

/// Deepest background — body / root surface.
pub const BG_BASE: u32 = 0x0A0A0A;

/// Editor and preview panel fill.
pub const BG_PANEL: u32 = 0x151515;

/// Sidebar and window chrome — "volcanic" surface.
pub const BG_SURFACE: u32 = 0x1A1A1A;

/// Subtle hover state overlay.
pub const BG_HOVER: u32 = 0x222222;

// ── Borders ───────────────────────────────────────────────────────────────────

/// Primary border — white/5 on BG_BASE.
pub const BORDER: u32 = 0x161616;

/// Secondary border — slightly more visible dividers.
pub const BORDER_SUBTLE: u32 = 0x2A2A2A;

// ── Ochre accent (#CC7722) ────────────────────────────────────────────────────

/// Full ochre — buttons, active indicators, logo bar.
pub const OCHRE: u32 = 0xCC7722;

/// ochre/10 — dimmed fill (selected tab bg, etc.).
pub const OCHRE_DIM: u32 = 0x201306;

/// ochre/20 — border-level accent.
pub const OCHRE_BORDER: u32 = 0x3D2008;

// ── Text ──────────────────────────────────────────────────────────────────────

/// Primary text — zinc-100.
pub const TEXT: u32 = 0xF4F4F5;

/// Secondary text — zinc-400.
pub const TEXT_MUTED: u32 = 0xA1A1AA;

/// Tertiary / placeholder text — zinc-500.
pub const TEXT_SUBTLE: u32 = 0x71717A;

/// Barely-visible text — zinc-600.
pub const TEXT_FAINT: u32 = 0x52525B;

// ── Mode indicator colors ─────────────────────────────────────────────────────

/// Insert mode — bright blue.
pub const MODE_INSERT: u32 = 0x528BFF;

/// Normal mode — ochre (matches accent brand).
pub const MODE_NORMAL: u32 = OCHRE;

/// Visual mode — purple.
pub const MODE_VISUAL: u32 = 0xA855F7;

// ── Cursor ────────────────────────────────────────────────────────────────────

/// Text color on top of a block cursor fill.
pub const CURSOR_FG: u32 = BG_BASE;
