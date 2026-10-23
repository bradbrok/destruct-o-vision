//! Native file dialogs. They're modal, so they run between UI frames, never
//! while one is being drawn.

use std::path::Path;

/// File extensions treated as movies when dropped on the rack. The decoder
/// has the last word on whether one plays.
const MOVIE_EXTENSIONS: [&str; 8] = ["mov", "mp4", "m4v", "qt", "3gp", "mts", "m2ts", "avi"];

pub fn is_movie(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| MOVIE_EXTENSIONS.iter().any(|m| m.eq_ignore_ascii_case(e)))
}

/// Asks for a movie file, starting in the folder of `current`. None if
/// cancelled.
#[cfg(target_os = "macos")]
pub fn choose_movie(current: &str) -> Option<String> {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSModalResponseOK, NSOpenPanel};
    use objc2_foundation::{NSArray, NSString, NSURL};

    let mtm = MainThreadMarker::new()?;
    let panel = NSOpenPanel::openPanel(mtm);
    panel.setCanChooseFiles(true);
    panel.setCanChooseDirectories(false);
    panel.setAllowsMultipleSelection(false);
    panel.setMessage(Some(&NSString::from_str("Choose a movie to play")));
    // Any type the system counts as a movie.
    let movies = NSArray::from_retained_slice(&[NSString::from_str("public.movie")]);
    #[allow(deprecated)]
    panel.setAllowedFileTypes(Some(&movies));
    if let Some(dir) = Path::new(current).parent().filter(|d| d.is_dir()) {
        let url = NSURL::fileURLWithPath(&NSString::from_str(&dir.to_string_lossy()));
        panel.setDirectoryURL(Some(&url));
    }
    if panel.runModal() != NSModalResponseOK {
        return None;
    }
    Some(panel.URL()?.path()?.to_string())
}

#[cfg(not(target_os = "macos"))]
pub fn choose_movie(_current: &str) -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn movies_are_known_by_extension() {
        assert!(is_movie(Path::new("/clips/Loop.MOV")));
        assert!(is_movie(Path::new("a.mp4")));
        assert!(!is_movie(Path::new("patch.dovpatch")));
        assert!(!is_movie(Path::new("module.wgsl")));
        assert!(!is_movie(Path::new("no_extension")));
    }
}
