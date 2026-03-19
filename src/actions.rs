/// Global action types for ockr.
///
/// Actions are dispatched through GPUI's keybinding system. Every user-visible
/// operation is represented as an action so it can appear in the Command Palette,
/// be bound to any key, and be composed into macros (Phase 2+).
use gpui::actions;

actions!(
    ockr,
    [
        // Application
        Quit,
        // Command palette
        OpenCommandPalette,
        // Vault / file operations
        NewNote,
        SaveFile,
        OpenVault,
        // Layout
        ToggleSidebar,
        SplitPaneVertical,
        SplitPaneHorizontal,
        // Navigation
        OpenQuickSwitch,
        VaultSearch,
    ]
);
