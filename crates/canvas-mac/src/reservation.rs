//! Explicit monitor reservation, with write-ahead restoration journal.
use canvas_control::{Display, Operation, Rect, Response, Window};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub displays: Vec<Display>,
    pub windows: Vec<Window>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Move {
    pub window: Window,
    pub destination: Rect,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plan {
    pub original_displays: Vec<Display>,
    pub reserved: Display,
    #[serde(default)]
    pub additional_reserved: Vec<Display>,
    pub destinations: Vec<Display>,
    pub moves: Vec<Move>,
}
impl Plan {
    pub fn reserved_displays(&self) -> impl Iterator<Item = &Display> {
        std::iter::once(&self.reserved).chain(self.additional_reserved.iter())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Journal {
    pub version: u32,
    pub owner_pid: u32,
    pub plan: Plan,
    pub attempted: usize,
    pub restored: BTreeSet<usize>,
}

fn intersects(rect: Rect, screen: Rect) -> bool {
    i64::from(rect.x) < i64::from(screen.x) + i64::from(screen.width)
        && i64::from(screen.x) < i64::from(rect.x) + i64::from(rect.width)
        && i64::from(rect.y) < i64::from(screen.y) + i64::from(screen.height)
        && i64::from(screen.y) < i64::from(rect.y) + i64::from(rect.height)
}
fn center_in(rect: Rect, screen: Rect) -> bool {
    let x = i64::from(rect.x) + i64::from(rect.width) / 2;
    let y = i64::from(rect.y) + i64::from(rect.height) / 2;
    x >= i64::from(screen.x)
        && y >= i64::from(screen.y)
        && x < i64::from(screen.x) + i64::from(screen.width)
        && y < i64::from(screen.y) + i64::from(screen.height)
}
fn desktop_bounds(displays: &[Display]) -> Option<Rect> {
    let first = displays.first()?;
    let mut left = i64::from(first.frame.x);
    let mut top = i64::from(first.frame.y);
    let mut right = left + i64::from(first.frame.width);
    let mut bottom = top + i64::from(first.frame.height);
    for display in &displays[1..] {
        left = left.min(i64::from(display.frame.x));
        top = top.min(i64::from(display.frame.y));
        right = right.max(i64::from(display.frame.x) + i64::from(display.frame.width));
        bottom = bottom.max(i64::from(display.frame.y) + i64::from(display.frame.height));
    }
    Some(Rect {
        x: i32::try_from(left).ok()?,
        y: i32::try_from(top).ok()?,
        width: u32::try_from(right - left).ok()?,
        height: u32::try_from(bottom - top).ok()?,
    })
}
/// Finder exposes the desktop background itself through Accessibility as one
/// untitled, immutable window spanning the complete display layout. It is not
/// an application window and remains in place while displays are reserved.
fn is_finder_desktop_surface(window: &Window, displays: &[Display]) -> bool {
    window.app == "Finder"
        && window.title.is_empty()
        && !window.movable
        && !window.resizable
        && Some(window.frame) == desktop_bounds(displays)
}
fn stacked_frames(area: Rect, windows: &[Window]) -> Result<Vec<Rect>, String> {
    windows
        .iter()
        .enumerate()
        .map(|(index, window)| {
            let width = window.frame.width.min(area.width);
            let height = window.frame.height.min(area.height);
            if width == 0 || height == 0 {
                return Err("Window has invalid dimensions".into());
            }
            let step = i64::try_from(index).unwrap_or(i64::MAX).saturating_mul(32);
            let x_range = i64::from(area.width - width) + 1;
            let y_range = i64::from(area.height - height) + 1;
            Ok(Rect {
                x: i32::try_from(i64::from(area.x) + step % x_range)
                    .map_err(|_| "Display coordinate overflow")?,
                y: i32::try_from(i64::from(area.y) + step % y_range)
                    .map_err(|_| "Display coordinate overflow")?,
                width,
                height,
            })
        })
        .collect()
}
/// Selected destinations and streaming displays form the explicit affected set.
/// Review contains every window moved, including existing destination windows.
pub fn plan(
    snapshot: &Snapshot,
    reserved_id: &str,
    destination_ids: &[String],
) -> Result<Plan, String> {
    plan_outputs(snapshot, &[reserved_id.into()], destination_ids)
}
/// Build one reviewed operation for one or more independently streamed outputs.
/// Display order comes from the current native inventory, not click order.
pub fn plan_outputs(
    snapshot: &Snapshot,
    reserved_ids: &[String],
    destination_ids: &[String],
) -> Result<Plan, String> {
    if reserved_ids.is_empty() {
        return Err("Choose at least one streaming display".into());
    }
    if destination_ids.is_empty() {
        return Err("A separate display is required to keep local windows available".into());
    }
    let requested: BTreeSet<_> = reserved_ids.iter().collect();
    if requested.len() != reserved_ids.len() {
        return Err("Choose each streaming display once".into());
    }
    let reserved: Vec<_> = snapshot
        .displays
        .iter()
        .filter(|display| requested.contains(&display.id))
        .cloned()
        .collect();
    if reserved.len() != reserved_ids.len() {
        return Err("Streaming display disconnected".into());
    }
    let mut unique = BTreeSet::new();
    let destinations: Vec<_> = destination_ids
        .iter()
        .map(|id| {
            if requested.contains(id) || !unique.insert(id) {
                return Err(
                    "Choose distinct destination displays, separate from the streaming display"
                        .into(),
                );
            }
            snapshot
                .displays
                .iter()
                .find(|d| d.id == *id)
                .cloned()
                .ok_or_else(|| "Destination display disconnected".to_string())
        })
        .collect::<Result<_, String>>()?;
    let selected = |w: &Window| {
        reserved
            .iter()
            .any(|display| intersects(w.frame, display.frame))
            || destinations.iter().any(|d| intersects(w.frame, d.frame))
    };
    let mut windows = Vec::new();
    for w in snapshot
        .windows
        .iter()
        .filter(|w| selected(w) && !w.minimized)
    {
        if is_finder_desktop_surface(w, &snapshot.displays) {
            continue;
        }
        if !w.movable || !w.resizable || w.fullscreen {
            return Err(format!("{} — {} cannot be rearranged; leave fullscreen or close the blocking window before reserving this display",w.app,w.title));
        }
        if w.app.eq_ignore_ascii_case("Moonlight") {
            return Err(
                "Close the existing Moonlight window before starting a managed stream".into(),
            );
        }
        windows.push(w.clone());
    }
    windows.sort_by(|a, b| a.id.cmp(&b.id));
    let mut moves = Vec::new();
    for (index, display) in destinations.iter().enumerate() {
        let group: Vec<_> = windows
            .iter()
            .skip(index)
            .step_by(destinations.len())
            .cloned()
            .collect();
        let frames = stacked_frames(display.visible_frame, &group)?;
        for (window, destination) in group.into_iter().zip(frames) {
            moves.push(Move {
                window,
                destination,
            });
        }
    }
    Ok(Plan {
        original_displays: snapshot.displays.clone(),
        reserved: reserved[0].clone(),
        additional_reserved: reserved[1..].to_vec(),
        destinations,
        moves,
    })
}

pub trait Desktop {
    fn snapshot(&mut self) -> Result<Snapshot, String>;
    fn set_frame(&mut self, id: &str, rect: Rect) -> Result<(), String>;
}
pub struct Native;
impl Desktop for Native {
    fn snapshot(&mut self) -> Result<Snapshot, String> {
        match canvas_control::request(Operation::Snapshot)? {
            Response::Snapshot { displays, windows } => Ok(Snapshot { displays, windows }),
            _ => Err("Unexpected Dock Control snapshot response".into()),
        }
    }
    fn set_frame(&mut self, id: &str, frame: Rect) -> Result<(), String> {
        match canvas_control::request(Operation::SetFrame {
            id: id.into(),
            frame,
        })? {
            Response::Applied { window } if window.frame == frame => Ok(()),
            _ => Err("Window did not accept the requested frame".into()),
        }
    }
}
struct JournalLock(fs::File);
impl JournalLock {
    fn acquire(path: &Path) -> Result<Self, String> {
        use std::os::{fd::AsRawFd, unix::fs::OpenOptionsExt};
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(path.with_extension("lock"))
            .map_err(|e| e.to_string())?;
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err("Another Dock process is changing or restoring these windows; try again after it finishes".into());
        }
        Ok(Self(file))
    }
}
impl Drop for JournalLock {
    fn drop(&mut self) {
        use std::os::fd::AsRawFd;
        unsafe {
            libc::flock(self.0.as_raw_fd(), libc::LOCK_UN);
        }
    }
}
fn save(path: &Path, journal: &Journal) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let temp = path.with_extension("tmp");
    let bytes = serde_json::to_vec_pretty(journal).map_err(|e| e.to_string())?;
    use std::io::Write;
    let mut file = fs::File::create(&temp).map_err(|e| e.to_string())?;
    file.write_all(&bytes)
        .and_then(|_| file.sync_all())
        .map_err(|e| e.to_string())?;
    fs::rename(temp, path).map_err(|e| e.to_string())?;
    if let Some(parent) = path.parent() {
        fs::File::open(parent)
            .and_then(|f| f.sync_all())
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}
fn load(path: &Path) -> Result<Journal, String> {
    let j: Journal = serde_json::from_slice(&fs::read(path).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    if j.version != 1 || j.attempted > j.plan.moves.len() {
        return Err("Invalid restoration journal".into());
    }
    Ok(j)
}
/// Restore attempted operations including a failed operation that partially moved.
/// Unknown helper-instance IDs stay unresolved rather than guessing a new window.
pub fn restore(backend: &mut impl Desktop, path: &Path) -> Result<(), String> {
    let _lock = JournalLock::acquire(path)?;
    restore_locked(backend, path)
}
fn restore_locked(backend: &mut impl Desktop, path: &Path) -> Result<(), String> {
    let mut j = load(path)?;
    if j.owner_pid != std::process::id() && unsafe { libc::kill(j.owner_pid as i32, 0) } == 0 {
        return Err(
            "Another Dock process owns this stream. Stop it there before restoring windows.".into(),
        );
    }
    let snapshot = backend.snapshot()?;
    let mut errors = Vec::new();
    for i in (0..j.attempted).rev() {
        if j.restored.contains(&i) {
            continue;
        }
        let movement = &j.plan.moves[i];
        let originals: Vec<_> = j
            .plan
            .original_displays
            .iter()
            .filter(|d| intersects(movement.window.frame, d.frame))
            .collect();
        if originals.is_empty()
            || !originals.iter().all(|original| {
                snapshot
                    .displays
                    .iter()
                    .any(|d| d.id == original.id && d.frame == original.frame)
            })
        {
            errors.push(format!(
                "Original display unavailable or changed for {}",
                movement.window.title
            ));
            continue;
        }
        if !snapshot
            .windows
            .iter()
            .any(|w| w.id == movement.window.id && w.pid == movement.window.pid)
        {
            errors.push(format!(
                "Window stale or closed: {} — {}",
                movement.window.app, movement.window.title
            ));
            continue;
        }
        match backend.set_frame(&movement.window.id, movement.window.frame) {
            Ok(()) => {
                j.restored.insert(i);
                if let Err(error) = save(path, &j) {
                    errors.push(format!("Could not persist recovery progress: {error}"));
                }
            }
            Err(e) => errors.push(format!("{}: {e}", movement.window.title)),
        }
    }
    if errors.is_empty() {
        fs::remove_file(path).map_err(|e| e.to_string())?;
        Ok(())
    } else {
        Err(format!(
            "Restore incomplete; recovery journal retained: {}",
            errors.join("; ")
        ))
    }
}
pub fn apply(backend: &mut impl Desktop, path: &Path, plan: &Plan) -> Result<(), String> {
    let _lock = JournalLock::acquire(path)?;
    if path.exists() {
        return Err("A previous stream needs window recovery before starting another".into());
    }
    let current = backend.snapshot()?;
    for display in plan.reserved_displays().chain(plan.destinations.iter()) {
        if !current.displays.iter().any(|d| {
            d.id == display.id
                && d.frame == display.frame
                && d.visible_frame == display.visible_frame
        }) {
            return Err("Display layout changed; review a new plan".into());
        }
    }
    for movement in &plan.moves {
        if !current.windows.iter().any(|w| {
            w.id == movement.window.id
                && w.pid == movement.window.pid
                && w.frame == movement.window.frame
        }) {
            return Err("Windows changed since review; review a new plan".into());
        }
    }
    // New source/destination windows also invalidate authorization for this snapshot.
    let reserved_ids = plan
        .reserved_displays()
        .map(|display| display.id.clone())
        .collect::<Vec<_>>();
    let fresh = crate::reservation::plan_outputs(
        &current,
        &reserved_ids,
        &plan
            .destinations
            .iter()
            .map(|d| d.id.clone())
            .collect::<Vec<_>>(),
    )?;
    if fresh.moves.len() != plan.moves.len()
        || fresh
            .reserved_displays()
            .map(|display| &display.id)
            .ne(plan.reserved_displays().map(|display| &display.id))
        || !fresh
            .moves
            .iter()
            .zip(&plan.moves)
            .all(|(a, b)| a.window.id == b.window.id && a.destination == b.destination)
    {
        return Err("Windows changed since review; review a new plan".into());
    }
    let mut journal = Journal {
        version: 1,
        owner_pid: std::process::id(),
        plan: plan.clone(),
        attempted: 0,
        restored: BTreeSet::new(),
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    use std::io::Write;
    let mut initial = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| format!("Could not reserve recovery journal: {e}"))?;
    initial
        .write_all(&serde_json::to_vec(&journal).map_err(|e| e.to_string())?)
        .and_then(|_| initial.sync_all())
        .map_err(|e| e.to_string())?;
    save(path, &journal)?;
    for (index, movement) in plan.moves.iter().enumerate() {
        journal.attempted = index + 1;
        if let Err(error) = save(path, &journal) {
            let recovery = restore_locked(backend, path);
            return Err(format!(
                "Could not journal the next move: {error}; recovery: {recovery:?}"
            ));
        }
        if let Err(error) = backend.set_frame(&movement.window.id, movement.destination) {
            let recovery = restore_locked(backend, path);
            return Err(format!(
                "Could not prepare display: {error}. {}",
                match recovery {
                    Ok(()) => "Original windows restored".into(),
                    Err(e) => e,
                }
            ));
        }
    }
    Ok(())
}

pub fn journal_path() -> Result<PathBuf, String> {
    Ok(
        PathBuf::from(std::env::var_os("HOME").ok_or("HOME is unset")?)
            .join("Library/Application Support/Dock/stream-recovery.json"),
    )
}
/// Child ownership stays in this process; after a crash we restore windows but
/// never kill a PID from disk that could have been reused for another process.
pub struct Stream {
    children: Vec<Child>,
    finished: bool,
    pub path: PathBuf,
}
impl Stream {
    pub fn stop(mut self) -> Result<(), String> {
        self.finished = true;
        for child in &mut self.children {
            let _ = child.kill();
            let _ = child.wait();
        }
        restore(&mut Native, &self.path)
    }
    pub fn ended(&mut self) -> Result<bool, String> {
        self.children.iter_mut().try_fold(false, |ended, child| {
            child
                .try_wait()
                .map(|status| ended || status.is_some())
                .map_err(|e| e.to_string())
        })
    }
}
pub fn start(plan: &Plan, host: &str) -> Result<Stream, String> {
    host.parse::<std::net::IpAddr>()
        .map_err(|_| "Stream host must be an IP address")?;
    let mut command = Command::new("/Applications/Moonlight.app/Contents/MacOS/Moonlight");
    command.args([
        "stream",
        host,
        "Desktop",
        "--1080",
        "--fps",
        "60",
        "--bitrate",
        "10000",
        "--display-mode",
        "windowed",
        "--absolute-mouse",
        "--capture-system-keys",
        "never",
    ]);
    if plan.additional_reserved.is_empty() {
        start_commands(plan, vec![command], "Moonlight")
    } else {
        Err("Moonlight reservations support one streaming display".into())
    }
}
/// Reserve selected Mac displays for matching project-desktop outputs. The
/// shared viewers are owned here; existing viewers and Linux apps survive.
pub fn start_project(plan: &Plan, project: &str) -> Result<Stream, String> {
    let mut commands =
        crate::remote::project_viewer_commands(project, plan.reserved_displays().count())?;
    // Connection failure must end the owned process, so the lifecycle observer
    // can restore local windows instead of leaving a reconnect dialog fullscreen.
    for (command, display) in commands.iter_mut().zip(plan.reserved_displays()) {
        command.args([
            "-AlertOnFatalError=0",
            "-ReconnectOnError=0",
            "-FullScreen=1",
            "-FullScreenMode",
            "Current",
            "-geometry",
            &viewer_geometry(display),
        ]);
    }
    start_commands(plan, commands, "TigerVNC")
}
fn viewer_geometry(display: &Display) -> String {
    let frame = display.visible_frame;
    format!("{}x{}{:+}{:+}", frame.width, frame.height, frame.x, frame.y)
}
fn start_commands(plan: &Plan, commands: Vec<Command>, label: &str) -> Result<Stream, String> {
    let path = journal_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    use std::os::unix::fs::OpenOptionsExt;
    let log = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path.with_file_name("reserved-stream.log"))
        .map_err(|e| e.to_string())?;
    let error_log = log.try_clone().map_err(|e| e.to_string())?;
    apply(&mut Native, &path, plan)?;
    let mut children = Vec::new();
    for mut command in commands {
        let launched = command
            .stdout(Stdio::from(log.try_clone().map_err(|e| e.to_string())?))
            .stderr(Stdio::from(
                error_log.try_clone().map_err(|e| e.to_string())?,
            ))
            .spawn();
        match launched {
            Ok(child) => children.push(child),
            Err(e) => {
                for child in &mut children {
                    let _ = child.kill();
                    let _ = child.wait();
                }
                let recovery = restore(&mut Native, &path);
                return Err(format!(
                    "{label} could not start: {e}; recovery: {recovery:?}"
                ));
            }
        }
    }
    if children.len() != plan.reserved_displays().count() {
        for child in &mut children {
            let _ = child.kill();
            let _ = child.wait();
        }
        let recovery = restore(&mut Native, &path);
        return Err(format!(
            "Viewer/output mapping changed before launch; recovery: {recovery:?}"
        ));
    }
    let configure: Result<(), String> = (|| {
        for (child, display) in children.iter_mut().zip(plan.reserved_displays()) {
            let start = Instant::now();
            let mut appeared = false;
            while start.elapsed() < Duration::from_secs(25) {
                if child.try_wait().map_err(|e| e.to_string())?.is_some() {
                    return Err(format!("{label} exited before its stream window appeared"));
                }
                let snapshot = Native.snapshot()?;
                let windows: Vec<_> = snapshot
                    .windows
                    .iter()
                    .filter(|w| {
                        w.pid == child.id()
                            && if label == "TigerVNC" {
                                !w.minimized
                            } else {
                                w.movable
                                    && w.resizable
                                    && !w.title.eq_ignore_ascii_case("Moonlight")
                            }
                    })
                    .collect();
                if windows.len() == 1 {
                    let w = windows[0];
                    if label == "TigerVNC" {
                        // TigerVNC uses a custom fullscreen window on macOS. It
                        // fills the display but does not publish AXFullScreen.
                        if w.frame == display.frame {
                            appeared = true;
                            break;
                        }
                    } else {
                        Native.set_frame(&w.id, display.visible_frame)?;
                        fullscreen(&w.id, display)?;
                        appeared = true;
                        break;
                    }
                }
                std::thread::sleep(Duration::from_millis(300));
            }
            if !appeared {
                return Err(format!(
                    "{label} stream window did not appear within 25 seconds"
                ));
            }
        }
        Ok(())
    })();
    if let Err(error) = configure {
        for child in &mut children {
            let _ = child.kill();
            let _ = child.wait();
        }
        let recovery = restore(&mut Native, &path);
        return Err(format!("{error}; window recovery: {recovery:?}"));
    }
    Ok(Stream {
        children,
        path,
        finished: false,
    })
}
fn fullscreen(id: &str, display: &Display) -> Result<(), String> {
    match canvas_control::request(Operation::SetFullscreen {
        id: id.into(),
        fullscreen: true,
    })? {
        Response::Applied { .. } => {}
        _ => return Err("Dock Control did not acknowledge fullscreen request".into()),
    }
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(4) {
        let snapshot = Native.snapshot()?;
        if snapshot
            .displays
            .iter()
            .any(|d| d.id == display.id && d.frame == display.frame)
            && snapshot
                .windows
                .iter()
                .any(|w| w.id == id && w.fullscreen && center_in(w.frame, display.frame))
        {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(150));
    }
    Err("The viewer did not enter fullscreen on the selected display".into())
}

impl Drop for Stream {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        for child in &mut self.children {
            let _ = child.kill();
            let _ = child.wait();
        }
        if self.path.exists() {
            if let Err(error) = restore(&mut Native, &self.path) {
                eprintln!("Dock window recovery requires attention: {error}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn display(id: &str, x: i32) -> Display {
        Display {
            id: id.into(),
            name: id.into(),
            frame: Rect {
                x,
                y: 0,
                width: 1920,
                height: 1080,
            },
            visible_frame: Rect {
                x,
                y: 25,
                width: 1920,
                height: 1055,
            },
            primary: x == 0,
        }
    }
    fn window(id: &str, x: i32) -> Window {
        Window {
            id: id.into(),
            pid: 42,
            app: "Editor".into(),
            title: id.into(),
            frame: Rect {
                x,
                y: 100,
                width: 800,
                height: 500,
            },
            movable: true,
            resizable: true,
            minimized: false,
            fullscreen: false,
        }
    }
    fn snapshot() -> Snapshot {
        Snapshot {
            displays: vec![display("left", -1920), display("right", 0)],
            windows: vec![window("a", -1800), window("b", 100)],
        }
    }
    struct Mock {
        snapshot: Snapshot,
        fail_once: Option<String>,
        calls: Vec<(String, Rect)>,
    }
    impl Desktop for Mock {
        fn snapshot(&mut self) -> Result<Snapshot, String> {
            Ok(self.snapshot.clone())
        }
        fn set_frame(&mut self, id: &str, frame: Rect) -> Result<(), String> {
            self.calls.push((id.into(), frame));
            self.snapshot
                .windows
                .iter_mut()
                .find(|w| w.id == id)
                .ok_or("stale")?
                .frame = frame;
            if self.fail_once.as_deref() == Some(id) {
                self.fail_once = None;
                return Err("partial native failure".into());
            }
            Ok(())
        }
    }
    fn temp() -> PathBuf {
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        std::env::temp_dir().join(format!(
            "dock-recovery-test-{}-{}.json",
            std::process::id(),
            N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ))
    }
    #[test]
    fn plan_includes_source_and_destination_windows_with_negative_origins() {
        let plan = plan(&snapshot(), "right", &["left".into()]).unwrap();
        assert_eq!(plan.moves.len(), 2);
        for m in plan.moves {
            assert!(m.destination.x < 0);
            assert!(center_in(m.destination, display("left", -1920).frame));
            assert_eq!(m.destination.width, m.window.frame.width);
            assert_eq!(m.destination.height, m.window.frame.height);
        }
    }
    #[test]
    fn single_display_or_same_destination_refused() {
        assert!(plan(&snapshot(), "left", &[]).is_err());
        assert!(plan(&snapshot(), "left", &["left".into()]).is_err());
    }
    #[test]
    fn multiple_streaming_displays_follow_native_inventory_order() {
        let mut current = snapshot();
        current.displays.push(display("third", 1920));
        let planned = plan_outputs(
            &current,
            &["third".into(), "right".into()],
            &["left".into()],
        )
        .unwrap();
        assert_eq!(
            planned
                .reserved_displays()
                .map(|display| display.id.as_str())
                .collect::<Vec<_>>(),
            ["right", "third"]
        );
        assert!(planned
            .moves
            .iter()
            .all(|movement| movement.destination.x < 0));
        let restored: Plan =
            serde_json::from_slice(&serde_json::to_vec(&planned).unwrap()).unwrap();
        assert_eq!(restored.additional_reserved.len(), 1);
    }
    #[test]
    fn viewer_geometry_uses_signed_native_display_coordinates() {
        assert_eq!(
            viewer_geometry(&display("left", -1920)),
            "1920x1055-1920+25"
        );
        assert_eq!(viewer_geometry(&display("right", 0)), "1920x1055+0+25");
    }
    #[test]
    fn finder_desktop_surface_is_not_a_reservation_blocker() {
        let mut current = snapshot();
        current.windows.push(Window {
            id: "desktop".into(),
            pid: 99,
            app: "Finder".into(),
            title: String::new(),
            frame: Rect {
                x: -1920,
                y: 0,
                width: 3840,
                height: 1080,
            },
            movable: false,
            resizable: false,
            minimized: false,
            fullscreen: false,
        });
        let planned = plan(&current, "left", &["right".into()]).unwrap();
        assert!(planned
            .moves
            .iter()
            .all(|movement| movement.window.id != "desktop"));

        current.windows.last_mut().unwrap().title = "Real Finder blocker".into();
        assert!(plan(&current, "left", &["right".into()]).is_err());
    }
    #[test]
    fn partial_move_failure_restores_even_failed_operation() {
        let initial = snapshot();
        let plan = plan(&initial, "left", &["right".into()]).unwrap();
        let mut backend = Mock {
            snapshot: initial.clone(),
            fail_once: Some("b".into()),
            calls: vec![],
        };
        let path = temp();
        assert!(apply(&mut backend, &path, &plan).is_err());
        assert_eq!(
            backend
                .snapshot
                .windows
                .iter()
                .map(|w| w.frame)
                .collect::<Vec<_>>(),
            initial.windows.iter().map(|w| w.frame).collect::<Vec<_>>()
        );
        assert!(!path.exists());
        assert_eq!(backend.calls.len(), 4);
    }
    #[test]
    fn disconnected_original_monitor_retains_recoverable_journal() {
        let initial = snapshot();
        let plan = plan(&initial, "left", &["right".into()]).unwrap();
        let mut backend = Mock {
            snapshot: initial,
            fail_once: None,
            calls: vec![],
        };
        let path = temp();
        apply(&mut backend, &path, &plan).unwrap();
        backend.snapshot.displays.retain(|d| d.id != "left");
        let error = restore(&mut backend, &path).unwrap_err();
        assert!(error.contains("Original display unavailable"), "{error}");
        assert!(path.exists());
        backend.snapshot.displays.push(display("left", -1920));
        restore(&mut backend, &path).unwrap();
        assert!(!path.exists());
    }
    #[test]
    fn stale_helper_ids_retained_without_guessing() {
        let initial = snapshot();
        let plan = plan(&initial, "left", &["right".into()]).unwrap();
        let mut backend = Mock {
            snapshot: initial,
            fail_once: None,
            calls: vec![],
        };
        let path = temp();
        apply(&mut backend, &path, &plan).unwrap();
        backend.snapshot.windows[0].id = "new-helper-id".into();
        let error = restore(&mut backend, &path).unwrap_err();
        assert!(error.contains("stale or closed"), "{error}");
        assert!(path.exists());
        fs::remove_file(path).unwrap();
    }
    #[test]
    fn newly_opened_window_invalidates_review() {
        let initial = snapshot();
        let plan = plan(&initial, "left", &["right".into()]).unwrap();
        let mut backend = Mock {
            snapshot: initial,
            fail_once: None,
            calls: vec![],
        };
        backend.snapshot.windows.push(window("new", -1200));
        assert!(apply(&mut backend, &temp(), &plan).is_err());
        assert!(backend.calls.is_empty());
    }
    #[test]
    fn straddling_source_window_is_included_even_when_center_is_elsewhere() {
        let mut snap = snapshot();
        snap.displays.push(display("third", 1920));
        snap.windows.push(window("straddler", 1800));
        let planned = plan(&snap, "third", &["left".into()]).unwrap();
        assert!(planned.moves.iter().any(|m| m.window.id == "straddler"));
    }
    #[test]
    fn journal_lock_excludes_competing_action_and_releases() {
        let path = temp();
        let lock = JournalLock::acquire(&path).unwrap();
        assert!(JournalLock::acquire(&path).is_err());
        drop(lock);
        assert!(JournalLock::acquire(&path).is_ok());
    }
    #[test]
    #[ignore = "moves real windows and opens two live Agora streams"]
    fn live_two_output_stream_restores_every_window() {
        assert_eq!(
            std::env::var("DOCK_LIVE_MULTI_STREAM_TEST").as_deref(),
            Ok("1"),
            "set DOCK_LIVE_MULTI_STREAM_TEST=1 to authorize the live desktop cycle"
        );
        let before = Native.snapshot().unwrap();
        let destination = before
            .displays
            .iter()
            .find(|display| display.primary)
            .expect("a primary display")
            .id
            .clone();
        let reserved = before
            .displays
            .iter()
            .filter(|display| !display.primary)
            .map(|display| display.id.clone())
            .collect::<Vec<_>>();
        assert_eq!(
            reserved.len(),
            2,
            "the live test requires two non-primary displays"
        );
        let plan = plan_outputs(&before, &reserved, &[destination]).unwrap();
        let originals = plan
            .moves
            .iter()
            .map(|movement| {
                (
                    movement.window.id.clone(),
                    movement.window.pid,
                    movement.window.frame,
                )
            })
            .collect::<Vec<_>>();

        let stream = start_project(&plan, "studio").unwrap();
        assert_eq!(stream.children.len(), 2);
        let active = Native.snapshot().unwrap();
        for (child, display) in stream.children.iter().zip(plan.reserved_displays()) {
            assert!(active
                .windows
                .iter()
                .any(|window| window.pid == child.id() && window.frame == display.frame));
        }
        stream.stop().unwrap();

        let restored = Native.snapshot().unwrap();
        for (id, pid, frame) in originals {
            let window = restored
                .windows
                .iter()
                .find(|window| window.id == id && window.pid == pid)
                .unwrap_or_else(|| panic!("window {id} disappeared during the stream cycle"));
            assert_eq!(window.frame, frame, "window {id} was not restored");
        }
        assert!(!journal_path().unwrap().exists());
    }
    #[test]
    #[ignore = "restores real windows from Dock's retained recovery journal"]
    fn live_restore_retained_journal() {
        assert_eq!(
            std::env::var("DOCK_LIVE_WINDOW_RECOVERY").as_deref(),
            Ok("1"),
            "set DOCK_LIVE_WINDOW_RECOVERY=1 to authorize live window recovery"
        );
        restore(&mut Native, &journal_path().unwrap()).unwrap();
    }
}
