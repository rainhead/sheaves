//! The running timer in the top bar: an icon, a label such as "Orcasound 1:30",
//! and a menu to stop or resume it.
//!
//! GNOME has no tray of its own. Ubuntu's AppIndicator extension draws items that
//! speak the StatusNotifierItem protocol over D-Bus, and it draws a label beside
//! the icon when an item has the Ayatana extension's `XAyatanaLabel` property.
//! The Rust crates for the protocol have no label, so the item is written here on
//! zbus, with the small part of the `com.canonical.dbusmenu` protocol a flat menu
//! of three entries needs.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use jiff::Timestamp;
use sheaves_core::hours::HoursFormat;
use sheaves_core::tracker::Activity;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::{ObjectPath, OwnedValue, Type, Value};
use zbus::{Connection, fdo, interface};

use crate::format;

const ITEM_PATH: &str = "/StatusNotifierItem";
const MENU_PATH: &str = "/MenuBar";
const WATCHER: &str = "org.kde.StatusNotifierWatcher";

/// Sheaves' own glyph, a sheaf of wheat between the plates of an hourglass, shown
/// while no timer runs. The same file is the Mac's idle glyph.
const GLYPH: &str = include_str!("../../../icons/sheaves-symbolic.svg");
const GLYPH_NAME: &str = "sheaves-symbolic";

/// Puts the glyph in `folder`, where the top bar is told to look for it, and
/// returns the folder. The name ends in `-symbolic`, so the shell draws it in the
/// bar's own colour, as it does its own icons.
pub fn install_glyph(folder: &Path) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(folder)?;
    let path = folder.join(format!("{GLYPH_NAME}.svg"));
    if std::fs::read_to_string(&path).ok().as_deref() != Some(GLYPH) {
        std::fs::write(&path, GLYPH)?;
    }
    Ok(folder.to_path_buf())
}

/// What a click in the top bar asks the app to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Open,
    StopOrResume,
    Quit,
}

/// How the item looks: what the app's state says the top bar should show.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Look {
    pub icon: String,
    /// Beside the icon. Empty when nothing runs, so the icon alone stays.
    pub label: String,
    /// The widest the label will grow while this timer runs, so the top bar can
    /// keep its width as the minutes tick.
    pub guide: String,
    pub tooltip: String,
    /// The first menu entry, "Stop …" or "Resume …", when there is something to
    /// stop or resume.
    pub toggle: Option<String>,
}

/// Longest project name shown in the top bar before it is shortened.
const LABEL_NAME_LIMIT: usize = 24;

impl Look {
    /// The look for `activity`. The icon alone carries the state, as the Mac's
    /// does, so a top bar that drops the label loses nothing but the detail: a
    /// pause button while a timer runs, a play button while one can be resumed.
    pub fn of(activity: &Activity, format: HoursFormat, now: Timestamp) -> Self {
        match activity {
            Activity::Idle => Look {
                icon: GLYPH_NAME.to_string(),
                label: String::new(),
                guide: String::new(),
                tooltip: "Sheaves: no timer running".to_string(),
                toggle: None,
            },
            Activity::Running(entry) => {
                let hours = format::hours(entry.hours(now), format);
                let name = shortened(&entry.project.name);
                Look {
                    icon: "media-playback-pause-symbolic".to_string(),
                    label: format!("{name} {hours}"),
                    guide: format!("{name} {}", widest(&hours)),
                    tooltip: format!("{} · {}, {hours}", entry.task.name, entry.project.name),
                    toggle: Some(format!("Stop {}", entry.task.name)),
                }
            }
            Activity::Recent(entry) => Look {
                icon: "media-playback-start-symbolic".to_string(),
                label: String::new(),
                guide: String::new(),
                tooltip: format!(
                    "Sheaves: {} · {} stopped",
                    entry.task.name, entry.project.name
                ),
                toggle: Some(format!("Resume {}", entry.task.name)),
            },
        }
    }
}

/// `hours` with every digit an 8, the widest in most fonts, and one digit more
/// than it has now, so `9:59` becoming `10:00` does not widen the label either.
fn widest(hours: &str) -> String {
    let eights: String = hours
        .chars()
        .map(|character| {
            if character.is_ascii_digit() {
                '8'
            } else {
                character
            }
        })
        .collect();
    format!("8{eights}")
}

fn shortened(name: &str) -> String {
    if name.chars().count() <= LABEL_NAME_LIMIT {
        return name.to_string();
    }
    let kept: String = name.chars().take(LABEL_NAME_LIMIT - 1).collect();
    format!("{}…", kept.trim_end())
}

/// A tooltip as the protocol carries it: an icon name, icon images, a title and
/// a body.
type ToolTip = (String, Vec<(i32, i32, Vec<u8>)>, String, String);

/// The menu's entries, by id. Ids stay fixed so a click on a menu drawn from an
/// older layout still means the same thing.
const TOGGLE: i32 = 1;
const SEPARATOR: i32 = 2;
const OPEN: i32 = 3;
const QUIT: i32 = 4;

#[derive(Default)]
struct Shared {
    look: Look,
    /// The folder holding the glyph, empty when it could not be written.
    icon_folder: String,
    /// Counts layout changes, which the menu protocol reports with each layout.
    revision: u32,
}

struct Item {
    shared: Arc<Mutex<Shared>>,
    commands: async_channel::Sender<Command>,
}

#[interface(name = "org.kde.StatusNotifierItem")]
impl Item {
    #[zbus(property)]
    fn category(&self) -> &str {
        "ApplicationStatus"
    }

    #[zbus(property)]
    fn id(&self) -> &str {
        "sheaves"
    }

    #[zbus(property)]
    fn title(&self) -> &str {
        "Sheaves"
    }

    #[zbus(property)]
    fn status(&self) -> &str {
        "Active"
    }

    #[zbus(property)]
    fn icon_name(&self) -> String {
        let shared = lock(&self.shared);
        // With no folder to find Sheaves' glyph in, a stock icon says idle instead.
        if shared.look.icon == GLYPH_NAME && shared.icon_folder.is_empty() {
            return "alarm-symbolic".to_string();
        }
        shared.look.icon.clone()
    }

    #[zbus(property)]
    fn icon_theme_path(&self) -> String {
        lock(&self.shared).icon_folder.clone()
    }

    #[zbus(property)]
    fn tool_tip(&self) -> ToolTip {
        let tooltip = lock(&self.shared).look.tooltip.clone();
        (String::new(), Vec::new(), "Sheaves".to_string(), tooltip)
    }

    #[zbus(property)]
    fn menu(&self) -> ObjectPath<'_> {
        ObjectPath::from_static_str_unchecked(MENU_PATH)
    }

    /// False, so a click opens the window and the menu waits for a right click.
    #[zbus(property)]
    fn item_is_menu(&self) -> bool {
        false
    }

    #[zbus(property, name = "XAyatanaLabel")]
    fn label(&self) -> String {
        lock(&self.shared).look.label.clone()
    }

    #[zbus(property, name = "XAyatanaLabelGuide")]
    fn label_guide(&self) -> String {
        lock(&self.shared).look.guide.clone()
    }

    fn activate(&self, _x: i32, _y: i32) {
        let _ = self.commands.try_send(Command::Open);
    }

    fn secondary_activate(&self, _x: i32, _y: i32) {
        let _ = self.commands.try_send(Command::StopOrResume);
    }

    fn context_menu(&self, _x: i32, _y: i32) {}

    fn scroll(&self, _delta: i32, _orientation: &str) {}

    #[zbus(signal)]
    async fn new_icon(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn new_tool_tip(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;

    #[zbus(signal, name = "XAyatanaNewLabel")]
    async fn new_label(emitter: &SignalEmitter<'_>, label: &str, guide: &str) -> zbus::Result<()>;
}

/// One entry of the menu and its children, as `com.canonical.dbusmenu` lays it
/// out: `(ia{sv}av)`, each child itself a layout inside a variant.
#[derive(Debug, Type, serde::Serialize)]
pub struct Layout {
    id: i32,
    properties: HashMap<String, OwnedValue>,
    children: Vec<Value<'static>>,
}

impl From<Layout> for Value<'static> {
    fn from(layout: Layout) -> Self {
        Value::from(
            zbus::zvariant::StructureBuilder::new()
                .add_field(layout.id)
                .add_field(layout.properties)
                .add_field(layout.children)
                .build()
                .expect("a layout has fields"),
        )
    }
}

fn text(value: &str) -> OwnedValue {
    Value::from(value.to_string())
        .try_into_owned()
        .expect("text holds no file descriptor")
}

/// The properties of entry `id`, or `None` for an entry the menu does not have.
fn properties(id: i32, look: &Look) -> Option<HashMap<String, OwnedValue>> {
    let mut properties = HashMap::new();
    match id {
        0 => {
            properties.insert("children-display".to_string(), text("submenu"));
        }
        TOGGLE => {
            properties.insert("label".to_string(), text(look.toggle.as_deref()?));
        }
        SEPARATOR => {
            look.toggle.as_ref()?;
            properties.insert("type".to_string(), text("separator"));
        }
        OPEN => {
            properties.insert("label".to_string(), text("Open Sheaves"));
        }
        QUIT => {
            properties.insert("label".to_string(), text("Quit"));
        }
        _ => return None,
    }
    Some(properties)
}

fn entries(look: &Look) -> Vec<i32> {
    [TOGGLE, SEPARATOR, OPEN, QUIT]
        .into_iter()
        .filter(|id| properties(*id, look).is_some())
        .collect()
}

fn layout(look: &Look) -> Layout {
    Layout {
        id: 0,
        properties: properties(0, look).unwrap_or_default(),
        children: entries(look)
            .into_iter()
            .map(|id| {
                Value::from(Layout {
                    id,
                    properties: properties(id, look).unwrap_or_default(),
                    children: Vec::new(),
                })
            })
            .collect(),
    }
}

struct Menu {
    shared: Arc<Mutex<Shared>>,
    commands: async_channel::Sender<Command>,
}

#[interface(name = "com.canonical.dbusmenu")]
impl Menu {
    #[zbus(property)]
    fn version(&self) -> u32 {
        3
    }

    #[zbus(property)]
    fn text_direction(&self) -> &str {
        "ltr"
    }

    #[zbus(property)]
    fn status(&self) -> &str {
        "normal"
    }

    #[zbus(property)]
    fn icon_theme_path(&self) -> Vec<String> {
        Vec::new()
    }

    /// The whole menu, whatever part was asked for: it is one level deep.
    fn get_layout(
        &self,
        _parent_id: i32,
        _recursion_depth: i32,
        _property_names: Vec<String>,
    ) -> (u32, Layout) {
        let shared = lock(&self.shared);
        (shared.revision, layout(&shared.look))
    }

    fn get_group_properties(
        &self,
        ids: Vec<i32>,
        _property_names: Vec<String>,
    ) -> Vec<(i32, HashMap<String, OwnedValue>)> {
        let look = lock(&self.shared).look.clone();
        let ids = if ids.is_empty() { entries(&look) } else { ids };
        ids.into_iter()
            .filter_map(|id| properties(id, &look).map(|properties| (id, properties)))
            .collect()
    }

    fn get_property(&self, id: i32, name: String) -> fdo::Result<OwnedValue> {
        let look = lock(&self.shared).look.clone();
        properties(id, &look)
            .and_then(|mut properties| properties.remove(&name))
            .ok_or_else(|| fdo::Error::InvalidArgs(format!("entry {id} has no {name}")))
    }

    fn event(&self, id: i32, event_id: &str, _data: OwnedValue, _timestamp: u32) {
        if event_id != "clicked" {
            return;
        }
        let command = match id {
            TOGGLE => Command::StopOrResume,
            OPEN => Command::Open,
            QUIT => Command::Quit,
            _ => return,
        };
        let _ = self.commands.try_send(command);
    }

    fn event_group(&self, events: Vec<(i32, String, OwnedValue, u32)>) -> Vec<i32> {
        for (id, event_id, data, timestamp) in events {
            self.event(id, &event_id, data, timestamp);
        }
        Vec::new()
    }

    fn about_to_show(&self, _id: i32) -> bool {
        false
    }

    fn about_to_show_group(&self, _ids: Vec<i32>) -> (Vec<i32>, Vec<i32>) {
        (Vec::new(), Vec::new())
    }

    #[zbus(signal)]
    async fn layout_updated(
        emitter: &SignalEmitter<'_>,
        revision: u32,
        parent: i32,
    ) -> zbus::Result<()>;
}

/// The item, registered with the top bar.
pub struct Indicator {
    connection: Connection,
    shared: Arc<Mutex<Shared>>,
}

impl Indicator {
    /// Puts the item in the top bar, and keeps it there when the shell restarts.
    /// Clicks arrive on `commands`; the glyph is looked for in `icon_folder`. Must
    /// run inside a Tokio runtime.
    pub async fn start(
        commands: async_channel::Sender<Command>,
        icon_folder: Option<PathBuf>,
    ) -> zbus::Result<Self> {
        let shared = Arc::new(Mutex::new(Shared {
            icon_folder: icon_folder
                .map(|folder| folder.to_string_lossy().into_owned())
                .unwrap_or_default(),
            ..Shared::default()
        }));
        let name = format!(
            "org.freedesktop.StatusNotifierItem-{}-1",
            std::process::id()
        );
        let connection = zbus::connection::Builder::session()?
            .name(name.clone())?
            .serve_at(
                ITEM_PATH,
                Item {
                    shared: shared.clone(),
                    commands: commands.clone(),
                },
            )?
            .serve_at(
                MENU_PATH,
                Menu {
                    shared: shared.clone(),
                    commands,
                },
            )?
            .build()
            .await?;
        tokio::spawn(stay_registered(connection.clone(), name));
        Ok(Self { connection, shared })
    }

    /// Shows `look`, telling the top bar only about what changed.
    pub async fn show(&self, look: Look) -> zbus::Result<()> {
        let (previous, revision) = {
            let mut shared = lock(&self.shared);
            if shared.look == look {
                return Ok(());
            }
            if shared.look.toggle != look.toggle {
                shared.revision += 1;
            }
            let previous = std::mem::replace(&mut shared.look, look.clone());
            (previous, shared.revision)
        };
        let item = SignalEmitter::new(&self.connection, ITEM_PATH)?;
        if previous.icon != look.icon {
            Item::new_icon(&item).await?;
        }
        if previous.tooltip != look.tooltip {
            Item::new_tool_tip(&item).await?;
        }
        if previous.label != look.label || previous.guide != look.guide {
            Item::new_label(&item, &look.label, &look.guide).await?;
        }
        if previous.toggle != look.toggle {
            let menu = SignalEmitter::new(&self.connection, MENU_PATH)?;
            Menu::layout_updated(&menu, revision, 0).await?;
        }
        Ok(())
    }
}

async fn register(connection: &Connection, name: &str) {
    let registered = connection
        .call_method(
            Some(WATCHER),
            "/StatusNotifierWatcher",
            Some(WATCHER),
            "RegisterStatusNotifierItem",
            &(name,),
        )
        .await;
    match registered {
        Ok(_) => log::info!("in the top bar as {name}"),
        Err(error) => {
            log::warn!("not in the top bar: {error}. Ubuntu's AppIndicator extension may be off")
        }
    }
}

/// Registers with the watcher now if it is there, and again each time it
/// returns. The watcher lives in GNOME Shell, so a shell restart or the extension
/// being turned off and on forgets every item.
///
/// The watch is set up before looking, so a watcher that appears in between, as
/// it can while the session is still starting at login, is not missed.
async fn stay_registered(connection: Connection, name: String) {
    let bus = match fdo::DBusProxy::new(&connection).await {
        Ok(bus) => bus,
        Err(error) => {
            log::warn!("cannot watch for the top bar: {error}");
            register(&connection, &name).await;
            return;
        }
    };
    let changes = bus
        .receive_name_owner_changed_with_args(&[(0, WATCHER)])
        .await;
    match bus
        .name_has_owner(WATCHER.try_into().expect("a valid bus name"))
        .await
    {
        Ok(true) => register(&connection, &name).await,
        _ => log::info!("waiting for the top bar's tray to appear"),
    }
    let Ok(mut changes) = changes else {
        return;
    };
    use futures_lite::StreamExt;
    while let Some(change) = changes.next().await {
        if let Ok(args) = change.args()
            && args.new_owner().is_some()
        {
            register(&connection, &name).await;
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sheaves_core::calendar_date::CalendarDate;
    use sheaves_core::models::Reference;
    use sheaves_core::tracked_entry::{EntryId, TrackedEntry};

    fn at(text: &str) -> Timestamp {
        text.parse().unwrap()
    }

    fn entry(project: &str, running: bool) -> TrackedEntry {
        TrackedEntry {
            id: EntryId::Server(1),
            project: Reference::new(1, project),
            task: Reference::new(2, "Programming"),
            client: None,
            notes: None,
            spent_date: CalendarDate::new(2026, 11, 3),
            banked_hours: 1.0,
            is_running: running,
            timer_started_at: running.then(|| at("2026-11-03T09:00:00Z")),
            is_locked: false,
            is_billable: true,
            is_pending: false,
            updated_at: at("2026-11-03T09:00:00Z"),
        }
    }

    #[test]
    fn a_running_timer_shows_its_project_and_hours() {
        let look = Look::of(
            &Activity::Running(entry("Orcasound", true)),
            HoursFormat::HoursMinutes,
            at("2026-11-03T09:30:00Z"),
        );
        assert_eq!(look.label, "Orcasound 1:30");
        assert_eq!(look.guide, "Orcasound 88:88");
        assert_eq!(look.icon, "media-playback-pause-symbolic");
        assert_eq!(look.toggle.as_deref(), Some("Stop Programming"));
    }

    #[test]
    fn a_stopped_timer_offers_to_resume_with_the_icon_alone() {
        let look = Look::of(
            &Activity::Recent(entry("Orcasound", false)),
            HoursFormat::HoursMinutes,
            at("2026-11-03T09:30:00Z"),
        );
        assert_eq!(look.label, "");
        assert_eq!(look.icon, "media-playback-start-symbolic");
        assert_eq!(look.toggle.as_deref(), Some("Resume Programming"));
    }

    /// The Mac's asset catalog needs its own copy of the glyph. This is where a
    /// change to one and not the other is caught.
    #[test]
    fn the_mac_shows_the_same_glyph() {
        let mac = include_str!(
            "../../../Apps/Mac/Resources/Assets.xcassets/Sheaves.imageset/sheaves-symbolic.svg"
        );
        assert_eq!(GLYPH, mac);
    }

    #[test]
    fn the_glyph_is_written_once() {
        let folder = tempfile::tempdir().unwrap();
        let icons = folder.path().join("icons");
        assert_eq!(install_glyph(&icons).unwrap(), icons);
        let path = icons.join("sheaves-symbolic.svg");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), GLYPH);
        let written = std::fs::metadata(&path).unwrap().modified().unwrap();
        install_glyph(&icons).unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().modified().unwrap(),
            written
        );
    }

    #[test]
    fn a_long_project_name_is_shortened() {
        assert_eq!(shortened("Orcasound"), "Orcasound");
        let long = "A project with a name far too long for the top bar";
        let short = shortened(long);
        assert_eq!(short.chars().count(), LABEL_NAME_LIMIT);
        assert!(short.ends_with('…'));
    }

    #[test]
    fn the_menu_offers_a_toggle_only_when_there_is_one() {
        let idle = Look::of(
            &Activity::Idle,
            HoursFormat::HoursMinutes,
            at("2026-11-03T09:30:00Z"),
        );
        assert_eq!(entries(&idle), [OPEN, QUIT]);
        let running = Look::of(
            &Activity::Running(entry("Orcasound", true)),
            HoursFormat::HoursMinutes,
            at("2026-11-03T09:30:00Z"),
        );
        assert_eq!(entries(&running), [TOGGLE, SEPARATOR, OPEN, QUIT]);
        assert_eq!(layout(&running).children.len(), 4);
    }
}
