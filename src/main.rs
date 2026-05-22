mod actions;
mod command;
mod compiler;
mod editor;
mod plugin;
mod session;
mod settings;
mod ui;
mod undo_store;
mod vault;

use actions::*;
use command::{CommandEntry, CommandRegistry};
use compiler::PreviewMode;
use gpui::{
    App, AppContext, Application, Bounds, KeyBinding, Menu, MenuItem, OsAction,
    PathPromptOptions, SharedString, TitlebarOptions, WindowBounds, WindowOptions, px, size,
};
use ui::theme::ThemePalette;
use vault::VaultState;

impl gpui::Global for PreviewMode {}

fn main() {
    // Handle CLI subcommands before launching the GUI.
    let args: Vec<String> = std::env::args().collect();
    if args.len() >= 2 {
        let vault_root = plugin::loader::detect_vault_root()
            .unwrap_or_else(|| std::env::current_dir().unwrap());
        match args[1].as_str() {
            "install" => {
                let url = match args.get(2) {
                    Some(u) => u.as_str(),
                    None => {
                        eprintln!("Usage: ockr install <url>");
                        std::process::exit(1);
                    }
                };
                match plugin::loader::install_plugin(&vault_root, url) {
                    Ok(entry) => println!("Installed: {} v{}", entry.id, entry.version),
                    Err(e) => { eprintln!("Error: {e}"); std::process::exit(1); }
                }
                return;
            }
            "update" => {
                match plugin::loader::update_plugins(&vault_root) {
                    Ok(()) => println!("Plugins updated."),
                    Err(e) => { eprintln!("Error: {e}"); std::process::exit(1); }
                }
                return;
            }
            "list" => {
                let lock = plugin::loader::Lockfile::load(&vault_root);
                if lock.plugins.is_empty() {
                    println!("No plugins installed.");
                } else {
                    println!("Installed plugins ({}):", lock.plugins.len());
                    for e in &lock.plugins {
                        println!("  {} v{}  ({})", e.id, e.version, e.url);
                    }
                }
                return;
            }
            "remove" => {
                let id = match args.get(2) {
                    Some(i) => i.as_str(),
                    None => {
                        eprintln!("Usage: ockr remove <plugin-id>");
                        std::process::exit(1);
                    }
                };
                match plugin::loader::remove_plugin(&vault_root, id) {
                    Ok(()) => println!("Removed plugin '{}'.", id),
                    Err(e) => { eprintln!("Error: {e}"); std::process::exit(1); }
                }
                return;
            }
            "help" | "--help" | "-h" => {
                println!("ockr — a Typst-native editor");
                println!("Usage:");
                println!("  ockr                         Launch the editor");
                println!("  ockr install <url>           Install a plugin from URL");
                println!("  ockr update                  Update all installed plugins");
                println!("  ockr list                    List installed plugins");
                println!("  ockr remove <plugin-id>      Remove an installed plugin");
                return;
            }
            _ => {}
        }
    }

    Application::new().run(|cx: &mut App| {
        #[cfg(target_os = "macos")]
        set_dock_icon();

        // Create the reactive vault entity (empty until a vault is opened).
        let vault = cx.new(|_| VaultState::empty());

        // Restore the last-opened vault from the session, if present.
        if let Some(last_path) = session::load_last_vault() {
            vault.update(cx, |state, _cx| {
                *state = VaultState::open(last_path);
            });
        }

        // Load settings: defaults ← global file ← vault file.
        let vault_root = vault.read(cx).root.clone();
        let user_settings = settings::load_settings(vault_root.as_deref());

        // Apply theme from settings.
        let theme = load_theme_by_name(&user_settings.theme);
        cx.set_global(theme);

        // Apply preview mode from settings.
        let preview = match user_settings.preview_mode.as_str() {
            "paged" => PreviewMode::Paged,
            _ => PreviewMode::Html,
        };
        cx.set_global(preview);

        // Store resolved settings as a GPUI global.
        cx.set_global(user_settings);

        // Initialize the command registry as a GPUI global.
        let mut registry = CommandRegistry::new();
        register_builtin_commands(&mut registry);
        cx.set_global(registry);

        // Keybindings.
        cx.bind_keys([
            KeyBinding::new("cmd-p", OpenCommandPalette, None),
            KeyBinding::new("cmd-shift-p", OpenCommandPalette, None),
            KeyBinding::new("cmd-o", OpenVault, None),
            KeyBinding::new("cmd-n", NewNote, None),
            KeyBinding::new("cmd-s", SaveFile, None),
            KeyBinding::new("cmd-b", ToggleSidebar, None),
            KeyBinding::new("ctrl-cmd-z", ToggleZenMode, None),
            KeyBinding::new("cmd-k", OpenQuickSwitch, None),
            KeyBinding::new("cmd-shift-r", OpenRecentFiles, None),
            KeyBinding::new("cmd-shift-k", OpenBacklinks, None),
            KeyBinding::new("cmd-shift-o", OpenOutline, None),
            KeyBinding::new("cmd-shift-f", OpenVaultSearch, None),
            KeyBinding::new("cmd-enter", FollowLink, None),
            KeyBinding::new("cmd-t", OpenDailyNote, None),
            KeyBinding::new("cmd-backslash", SplitPaneVertical, None),
            KeyBinding::new("cmd-shift-backslash", SplitPaneHorizontal, None),
            KeyBinding::new("cmd-w", BufferClose, None),
            KeyBinding::new("cmd-shift-}", BufferNext, None),
            KeyBinding::new("cmd-shift-{", BufferPrevious, None),
            KeyBinding::new("ctrl-h", FocusPaneLeft, None),
            KeyBinding::new("ctrl-l", FocusPaneRight, None),
            KeyBinding::new("ctrl-k", FocusPaneUp, None),
            KeyBinding::new("ctrl-j", FocusPaneDown, None),
            KeyBinding::new("cmd-q", Quit, None),
            KeyBinding::new("cmd-alt-h", TogglePreviewMode, None),
            KeyBinding::new("cmd-shift-e", ExportPdf, None),
            KeyBinding::new("cmd-shift-g", OpenGraphView, None),
            KeyBinding::new("cmd-f", OpenSearch, None),
            KeyBinding::new("cmd-h", OpenReplace, None),
        ]);

        // macOS menu bar.
        cx.set_menus(vec![
            Menu {
                name: "ockr".into(),
                items: vec![
                    MenuItem::action("About ockr", Quit), // placeholder
                    MenuItem::separator(),
                    MenuItem::action("Quit ockr", Quit),
                ],
            },
            Menu {
                name: "File".into(),
                items: vec![
                    MenuItem::action("New Note", NewNote),
                    MenuItem::action("Open Vault…", OpenVault),
                    MenuItem::separator(),
                    MenuItem::os_action("Save", SaveFile, OsAction::Undo), // Cmd-S handled by keybinding
                    MenuItem::action("Export PDF", ExportPdf),
                    MenuItem::separator(),
                    MenuItem::action("Close Tab", BufferClose),
                ],
            },
            Menu {
                name: "Edit".into(),
                items: vec![
                    MenuItem::os_action("Undo", Quit, OsAction::Undo),
                    MenuItem::os_action("Redo", Quit, OsAction::Redo),
                    MenuItem::separator(),
                    MenuItem::os_action("Cut", Quit, OsAction::Cut),
                    MenuItem::os_action("Copy", Quit, OsAction::Copy),
                    MenuItem::os_action("Paste", Quit, OsAction::Paste),
                    MenuItem::os_action("Select All", Quit, OsAction::SelectAll),
                    MenuItem::separator(),
                    MenuItem::action("Find…", OpenSearch),
                    MenuItem::action("Find & Replace…", OpenReplace),
                ],
            },
            Menu {
                name: "View".into(),
                items: vec![
                    MenuItem::action("Toggle Sidebar", ToggleSidebar),
                    MenuItem::action("Toggle Preview Mode", TogglePreviewMode),
                    MenuItem::action("Zen Mode", ToggleZenMode),
                    MenuItem::separator(),
                    MenuItem::submenu(Menu {
                        name: "Line Numbers".into(),
                        items: vec![
                            MenuItem::action("Relative", LineNumbersRelative),
                            MenuItem::action("Absolute", LineNumbersAbsolute),
                            MenuItem::action("Off", LineNumbersOff),
                        ],
                    }),
                    MenuItem::separator(),
                    MenuItem::action("Command Palette", OpenCommandPalette),
                ],
            },
            Menu {
                name: "Go".into(),
                items: vec![
                    MenuItem::action("Quick Switch", OpenQuickSwitch),
                    MenuItem::action("Recent Files", OpenRecentFiles),
                    MenuItem::action("Daily Note", OpenDailyNote),
                    MenuItem::separator(),
                    MenuItem::action("Graph View", OpenGraphView),
                    MenuItem::action("Backlinks", OpenBacklinks),
                    MenuItem::action("Document Outline", OpenOutline),
                    MenuItem::action("Vault Search", OpenVaultSearch),
                ],
            },
            Menu {
                name: "Window".into(),
                items: vec![
                    MenuItem::action("Split Vertical", SplitPaneVertical),
                    MenuItem::action("Split Horizontal", SplitPaneHorizontal),
                    MenuItem::separator(),
                    MenuItem::action("Focus Left", FocusPaneLeft),
                    MenuItem::action("Focus Right", FocusPaneRight),
                    MenuItem::action("Focus Up", FocusPaneUp),
                    MenuItem::action("Focus Down", FocusPaneDown),
                ],
            },
            Menu {
                name: "Plugins".into(),
                items: vec![
                    MenuItem::action("Plugin Manager", OpenPluginManager),
                ],
            },
        ]);

        // App-level action handlers.
        let vault_for_open = vault.clone();
        cx.on_action(move |_: &OpenVault, cx| {
            let vault = vault_for_open.clone();
            let rx = cx.prompt_for_paths(PathPromptOptions {
                files: false,
                directories: true,
                multiple: false,
                prompt: Some("Open Vault".into()),
            });
            cx.spawn(async move |cx| {
                // rx resolves to Result<Result<Option<Vec<PathBuf>>, Error>, Canceled>
                if let Ok(Ok(Some(paths))) = rx.await {
                    if let Some(path) = paths.into_iter().next() {
                        session::save_last_vault(&path);
                        cx.update(|cx| {
                            vault.update(cx, |state, cx| {
                                *state = VaultState::open(path);
                                cx.notify();
                            });
                        })
                        .ok();
                    }
                }
            })
            .detach();
        });

        cx.on_action(|_: &OpenCommandPalette, _cx| {
            // Story 08: launch Command Palette UI
        });
        cx.on_action(|_: &NewNote, _cx| {
            // Story 02+: create new note in vault
        });
        cx.on_action(|_: &SaveFile, _cx| {
            // Story 06: save active file
        });
        cx.on_action(|_: &OpenQuickSwitch, _cx| {
            // Story 11: quick note switcher
        });
        cx.on_action(|_: &OpenRecentFiles, _cx| {
            // Story 44: recent files modal
        });
        cx.on_action(|_: &OpenOutline, _cx| {
            // Story 34: document outline panel
        });
        cx.on_action(|_: &ToggleZenMode, _cx| {
            // Story 35: zen mode
        });
        cx.on_action(|_: &OpenVaultSearch, _cx| {
            // Story 02+: vault-wide full-text search
        });
        cx.on_action(|_: &Quit, cx| cx.quit());
        cx.on_action(|_: &ForceQuit, cx| cx.quit());
        cx.on_action(|_: &SaveFileAndQuit, _cx| {
            // TODO: trigger save on active editor then quit
        });
        cx.on_action(|_: &ReloadFile, _cx| {
            // TODO: reload active file from disk
        });
        cx.on_action(|_: &BufferNext, _cx| {});
        cx.on_action(|_: &BufferPrevious, _cx| {});
        cx.on_action(|_: &BufferClose, _cx| {});

        // Quit when the last window closes.
        cx.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();

        // Open main window.
        let bounds = Bounds::centered(None, size(px(1280.0), px(800.0)), cx);
        let vault_for_window = vault.clone();
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some(SharedString::from("ockr")),
                    ..Default::default()
                }),
                ..Default::default()
            },
            move |window, cx| {
                let entity = cx.new(|cx| {
                    let view = ui::MainWindow::new(vault_for_window, cx);
                    view.focus_handle.focus(window);
                    view
                });
                entity.update(cx, |view, cx| view.restore_session_tabs(cx));
                entity
            },
        )
        .unwrap();

        cx.activate(true);
    });
}

/// Set the macOS Dock icon from the embedded 1024×1024 PNG.
///
/// When running outside a `.app` bundle, macOS shows a generic Terminal icon.
/// This replaces it with the ockr `o|` icon at startup.
#[cfg(target_os = "macos")]
fn set_dock_icon() {
    use objc2::AnyThread;
    use objc2::rc::Retained;
    use objc2_app_kit::{NSApplication, NSImage};
    use objc2_foundation::{MainThreadMarker, NSData};

    const ICON_PNG: &[u8] =
        include_bytes!("../assets/ockr.iconset/icon_512x512@2x.png");

    unsafe {
        let mtm = MainThreadMarker::new_unchecked();
        let data = NSData::with_bytes(ICON_PNG);
        if let Some(image) = NSImage::initWithData(NSImage::alloc(), &data) {
            let app = NSApplication::sharedApplication(mtm);
            app.setApplicationIconImage(Some(&image));
            let _: Retained<NSImage> = image;
        }
    }
}

/// Load a theme by name, checking bundled themes and `~/.config/ockr/themes/`.
pub fn load_theme_by_name(name: &str) -> ThemePalette {
    // Check user themes directory first.
    let user_dir = dirs::config_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("~/.config"))
        .join("ockr")
        .join("themes");
    let user_path = user_dir.join(format!("{name}.toml"));
    if user_path.exists() {
        if let Ok(text) = std::fs::read_to_string(&user_path) {
            if let Ok(theme) = ThemePalette::from_toml(&text) {
                return theme;
            }
        }
    }

    // Bundled themes.
    match name {
        "ochre" => ThemePalette::from_toml(include_str!("../themes/ochre.toml"))
            .unwrap_or_else(|_| ThemePalette::oxide()),
        _ => ThemePalette::oxide(),
    }
}

fn register_builtin_commands(registry: &mut CommandRegistry) {
    // Each entry: (id, display name, keybinding hint shown in the picker).
    // Helix `:command` names are listed first so they appear when typing
    // short-form prefixes (e.g. typing "w" surfaces "write").
    let cmds: &[(&'static str, &'static str, Option<&'static str>)] = &[
        // ── Helix :commands ──────────────────────────────────────────────────
        ("write",           "write  · save file",                   Some(":w")),
        ("write-quit",      "write-quit  · save and quit",          Some(":wq")),
        ("quit",            "quit  · close ockr",                   Some(":q")),
        ("quit-force",      "quit!  · quit without saving",         Some(":q!")),
        ("reload",          "reload  · discard changes, reload",    Some(":reload")),
        ("open",            "open  · open vault / file",            Some(":o")),
        ("new",             "new  · new note",                      Some(":new")),
        ("buffer-next",     "buffer-next  · next open buffer",      Some(":bn")),
        ("buffer-previous", "buffer-previous  · previous buffer",   Some(":bp")),
        ("buffer-close",    "buffer-close  · close current buffer", Some(":bc")),
        ("toggle-sidebar",  "toggle-sidebar",                       Some(":toggle-sidebar")),
        ("toggle-zen-mode", "Zen Mode",                             Some("Ctrl-Cmd-Z")),
        // ── GUI commands (Cmd-* shortcuts) ───────────────────────────────────
        ("open-command-palette", "Open Command Palette",            Some("Cmd-P / :")),
        ("open-vault",           "Open Vault",                      Some("Cmd-O")),
        ("new-note",             "New Note",                        Some("Cmd-N")),
        ("save-file",            "Save File",                       Some("Cmd-S")),
        ("open-quick-switch",    "Quick Switch",                    Some("Cmd-K")),
        ("open-recent-files",    "Recent Files",                    Some("Cmd-Shift-R")),
        ("open-backlinks",       "Open Backlinks",                  Some("Cmd-Shift-K")),
        ("open-outline",         "Document Outline",                Some("Cmd-Shift-O")),
        ("vault-search",         "Vault Search",                    Some("Cmd-Shift-F")),
        ("open-search",          "Search",                          Some("Cmd-F")),
        ("open-replace",         "Find & Replace",                  Some("Cmd-H")),
        ("follow-link",          "Follow Link",                     Some("Cmd-Enter")),
        ("open-daily-note",      "Open Daily Note",                 Some("Cmd-T")),
        ("split-pane-vertical",  "Split Pane Vertical",             Some("Cmd-\\")),
        ("split-pane-horizontal","Split Pane Horizontal",           Some("Cmd-Shift-\\")),
        ("close-pane",           "Close Pane",                      Some("Cmd-W")),
        ("focus-pane-left",      "Focus Pane Left",                 Some("Ctrl-H")),
        ("focus-pane-right",     "Focus Pane Right",                Some("Ctrl-L")),
        ("focus-pane-up",        "Focus Pane Up",                   Some("Ctrl-K")),
        ("focus-pane-down",      "Focus Pane Down",                 Some("Ctrl-J")),
        ("toggle-preview-mode",  "Toggle Preview Mode (HTML / PDF)", Some("Cmd-Opt-H")),
        ("export-pdf",           "Export PDF",                       Some("Cmd-Shift-E")),
        ("open-graph-view",      "Graph View",                       Some("Cmd-Shift-G")),
        ("open-plugin-manager",  "Plugin Manager",                   None),
        // Editor display
        ("line-numbers-relative", "Line Numbers: Relative",          Some(":set nu rel")),
        ("line-numbers-absolute", "Line Numbers: Absolute",          Some(":set nu abs")),
        ("line-numbers-off",      "Line Numbers: Off",               Some(":set nonu")),
        // Settings
        ("reload-settings",       "Reload Settings",                  None),
        ("switch-keyboard-mode",  "Switch Keyboard Mode",             None),
        ("switch-theme",          "Switch Theme",                     None),
        ("toggle-typewriter-mode", "Toggle Typewriter Mode",           None),
        ("noh",                   "noh  · clear search highlights",   Some(":noh")),
        ("nohlsearch",            "nohlsearch  · clear search highlights", Some(":nohlsearch")),
    ];
    for &(id, name, hint) in cmds {
        registry.register(CommandEntry::new(id, name, hint, |_cx| {}));
    }
}
