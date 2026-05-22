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
        ForceQuit,
        // Command palette
        OpenCommandPalette,
        // Vault / file operations
        NewNote,
        SaveFile,
        SaveFileAndQuit,
        ReloadFile,
        OpenVault,
        // Layout
        ToggleSidebar,
        SplitPaneVertical,
        SplitPaneHorizontal,
        ClosePane,
        // Pane focus navigation (Ctrl-H/J/K/L)
        FocusPaneLeft,
        FocusPaneRight,
        FocusPaneUp,
        FocusPaneDown,
        // Navigation
        OpenQuickSwitch,
        OpenFilePicker,
        OpenRecentFiles,
        OpenBacklinks,
        OpenOutline,
        ToggleZenMode,
        OpenVaultSearch,
        FollowLink,
        OpenDailyNote,
        BufferNext,
        BufferPrevious,
        BufferClose,
        // Preview / export
        TogglePreviewMode,
        ExportPdf,
        // Graph view
        OpenGraphView,
        // Editor display
        LineNumbersRelative,
        LineNumbersAbsolute,
        LineNumbersOff,
        // In-buffer search / replace
        OpenSearch,
        OpenReplace,
        // Plugin management
        OpenPluginManager,
    ]
);
