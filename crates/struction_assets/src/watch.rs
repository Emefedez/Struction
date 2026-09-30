//! "Open in…": hand a source file to its full application, then recompile it
//! whenever it is saved and ask the asset server to reload the compiled result.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime};

use bevy::asset::AssetPath;
use bevy::prelude::*;

use crate::blender::Blender;
use crate::compile::{CompileSettings, CompileStatus, compile_asset_with};
use crate::error::AssetError;

/// The desktop's handler for files without a per-extension command.
const DEFAULT_OPENER: &str = if cfg!(target_os = "macos") {
    "open"
} else {
    "xdg-open"
};

/// Which program opens a source file: a per-extension command, else the default.
#[derive(Resource, Debug, Clone)]
pub struct OpenIn {
    /// Program and leading arguments; the file path is appended.
    pub default_command: Vec<String>,
    /// Lowercase extension without the dot (`"blend"`) to command.
    pub by_extension: HashMap<String, Vec<String>>,
}

impl Default for OpenIn {
    fn default() -> Self {
        Self {
            default_command: vec![DEFAULT_OPENER.into()],
            // The same executable the import pipeline runs; the macOS app bundle is not on `PATH`.
            by_extension: HashMap::from([(
                "blend".into(),
                vec![Blender::default().executable.to_string_lossy().into_owned()],
            )]),
        }
    }
}

impl OpenIn {
    pub fn command_for(&self, path: &Path) -> &[String] {
        path.extension()
            .and_then(|extension| extension.to_str())
            .and_then(|extension| self.by_extension.get(&extension.to_ascii_lowercase()))
            .unwrap_or(&self.default_command)
    }

    /// Starts the application detached from the editor's terminal.
    pub fn open(&self, path: &Path) -> Result<Child, AssetError> {
        let command = self.command_for(path);
        let Some((program, args)) = command.split_first() else {
            return Err(AssetError::io(
                path,
                std::io::Error::new(std::io::ErrorKind::InvalidInput, "empty open command"),
            ));
        };
        Command::new(program)
            .args(args)
            .arg(path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| AssetError::io(path, error))
    }
}

/// Sent when a watched source finished recompiling after a save.
#[derive(Message, Debug, Clone)]
pub struct SourceRecompiled {
    pub source: PathBuf,
    pub output: PathBuf,
    /// Asset path of `output`, when it lives under the asset server's root.
    pub asset_path: Option<AssetPath<'static>>,
    /// `UpToDate` when the file was touched without changing its content.
    pub result: Result<CompileStatus, String>,
}

/// Polls watched sources (size and modification time) and recompiles them on a
/// background thread once a change has been stable for one poll.
#[derive(Resource)]
pub struct SourceWatcher {
    pub poll_interval: Duration,
    pub settings: CompileSettings,
    sources: Vec<WatchedSource>,
    last_poll: Option<Instant>,
}

struct WatchedSource {
    source: PathBuf,
    output: PathBuf,
    asset_path: Option<AssetPath<'static>>,
    /// Stamp of the version last compiled (or seen when watching started).
    compiled: Option<FileStamp>,
    /// A new stamp waiting to be seen again before compiling.
    pending: Option<FileStamp>,
    job: Option<JoinHandle<Result<CompileStatus, AssetError>>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileStamp {
    modified: SystemTime,
    len: u64,
}

impl FileStamp {
    fn read(path: &Path) -> Option<Self> {
        let metadata = std::fs::metadata(path).ok()?;
        Some(Self {
            modified: metadata.modified().ok()?,
            len: metadata.len(),
        })
    }
}

impl Default for SourceWatcher {
    fn default() -> Self {
        Self::new(CompileSettings::default())
    }
}

impl SourceWatcher {
    pub fn new(settings: CompileSettings) -> Self {
        Self {
            poll_interval: Duration::from_millis(500),
            settings,
            sources: Vec::new(),
            last_poll: None,
        }
    }

    /// Recompiles `source` into `output` whenever it changes from now on.
    pub fn watch(
        &mut self,
        source: impl Into<PathBuf>,
        output: impl Into<PathBuf>,
        asset_path: Option<AssetPath<'static>>,
    ) {
        let source = source.into();
        self.unwatch(&source);
        self.sources.push(WatchedSource {
            compiled: FileStamp::read(&source),
            source,
            output: output.into(),
            asset_path,
            pending: None,
            job: None,
        });
    }

    /// Stops watching; a compile already running still finishes silently.
    pub fn unwatch(&mut self, source: &Path) {
        self.sources.retain(|watched| watched.source != source);
    }

    pub fn is_watching(&self, source: &Path) -> bool {
        self.sources.iter().any(|watched| watched.source == source)
    }

    /// Whether any recompile is running.
    pub fn is_busy(&self) -> bool {
        self.sources.iter().any(|watched| watched.job.is_some())
    }

    fn poll(&mut self, now: Instant, messages: &mut MessageWriter<SourceRecompiled>) {
        for watched in &mut self.sources {
            if watched.job.as_ref().is_some_and(JoinHandle::is_finished) {
                let job = watched.job.take().expect("checked above");
                let result = job
                    .join()
                    .unwrap_or_else(|_| Err(AssetError::io(&watched.source, panicked())));
                messages.write(SourceRecompiled {
                    source: watched.source.clone(),
                    output: watched.output.clone(),
                    asset_path: watched.asset_path.clone(),
                    result: result.map_err(|error| error.to_string()),
                });
            }
        }

        if self
            .last_poll
            .is_some_and(|last| now.duration_since(last) < self.poll_interval)
        {
            return;
        }
        self.last_poll = Some(now);

        for watched in &mut self.sources {
            if watched.job.is_some() {
                continue;
            }
            let stamp = FileStamp::read(&watched.source);
            if stamp.is_none() || stamp == watched.compiled {
                watched.pending = None;
                continue;
            }
            // Editors may still be writing: wait until the stamp holds for a poll.
            if watched.pending != stamp {
                watched.pending = stamp;
                continue;
            }
            watched.pending = None;
            watched.compiled = stamp;
            let (source, output, settings) = (
                watched.source.clone(),
                watched.output.clone(),
                self.settings.clone(),
            );
            watched.job = Some(std::thread::spawn(move || {
                compile_asset_with(&source, &output, &settings)
            }));
        }
    }
}

fn panicked() -> std::io::Error {
    std::io::Error::other("compile thread panicked")
}

pub fn poll_sources(
    mut watcher: ResMut<SourceWatcher>,
    mut messages: MessageWriter<SourceRecompiled>,
) {
    watcher.poll(Instant::now(), &mut messages);
}

/// Hot reload: reloads compiled outputs that the asset server knows by path.
pub fn reload_recompiled(
    mut messages: MessageReader<SourceRecompiled>,
    asset_server: Option<Res<AssetServer>>,
) {
    for message in messages.read() {
        match (&message.result, &message.asset_path, &asset_server) {
            (Ok(CompileStatus::Compiled), Some(path), Some(server)) => server.reload(path.clone()),
            (Err(error), ..) => warn!("recompiling {}: {error}", message.source.display()),
            _ => {}
        }
    }
}

/// Watches sources for "Open in…" and hot-reloads their compiled outputs.
pub struct SourceWatcherPlugin;

impl Plugin for SourceWatcherPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<SourceRecompiled>()
            .init_resource::<SourceWatcher>()
            .init_resource::<OpenIn>()
            .add_systems(Update, (poll_sources, reload_recompiled).chain());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chooses_command_by_extension() {
        let open_in = OpenIn::default();
        let blender = Blender::default().executable.to_string_lossy().into_owned();
        assert_eq!(open_in.command_for(Path::new("a/ogre.BLEND")), [blender]);
        assert_eq!(
            open_in.command_for(Path::new("notes.txt")),
            [DEFAULT_OPENER]
        );
    }

    #[test]
    fn opens_with_the_configured_command() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("opened");
        // `sh -c 'touch "$0"' <file>` stands in for the application.
        let open_in = OpenIn {
            default_command: vec!["sh".into(), "-c".into(), "touch \"$0\"".into()],
            by_extension: HashMap::new(),
        };
        let status = open_in.open(&marker).unwrap().wait().unwrap();
        assert!(status.success());
        assert!(marker.is_file());

        let missing = OpenIn {
            default_command: vec!["/nonexistent/editor".into()],
            by_extension: HashMap::new(),
        };
        assert!(missing.open(&marker).is_err());
    }
}
