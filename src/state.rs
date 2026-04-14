use crate::args::Status;
use crate::tool::Tool;

pub struct StateDetector {
    _tool: Tool,
}

impl StateDetector {
    pub fn new(tool: Tool) -> Self {
        Self { _tool: tool }
    }

    pub fn detect(&mut self, screen_text: &str, _tool_title_seen: bool) -> Status {
        let normalized = normalize(screen_text);

        if contains_any(&normalized, &["error", "failed", "traceback", "panic"]) {
            Status::Error
        } else if is_ready(&normalized) {
            Status::Ready
        } else if is_busy(&normalized) {
            Status::Busy
        } else if normalized.trim().is_empty() {
            Status::Starting
        } else {
            Status::Ready
        }
    }
}

fn is_ready(normalized: &str) -> bool {
    contains_any(
        normalized,
        &["enter to send", "tab to queue message", "tab to queue"],
    )
}

fn is_busy(normalized: &str) -> bool {
    contains_any(normalized, &["esc to interrupt", "press esc to interrupt"])
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

fn normalize(input: &str) -> String {
    input
        .to_lowercase()
        .replace('\r', "\n")
        .chars()
        .map(|ch| {
            if ch.is_control() && ch != '\n' {
                ' '
            } else {
                ch
            }
        })
        .collect::<String>()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefers_ready_when_prompt_is_visible() {
        let mut detector = StateDetector::new(Tool::Codex);
        let state = detector.detect("Starting MCP servers...\n> \nEnter to send", true);
        assert_eq!(state, Status::Ready);
    }

    #[test]
    fn prefers_ready_when_queue_message_hint_is_visible() {
        let mut detector = StateDetector::new(Tool::Codex);
        let state = detector.detect("Tab to queue message", true);
        assert_eq!(state, Status::Ready);
    }

    #[test]
    fn ready_hint_overrides_interrupt_hint() {
        let mut detector = StateDetector::new(Tool::Codex);
        let state = detector.detect("Esc to interrupt\nTab to queue message", true);
        assert_eq!(state, Status::Ready);
    }

    #[test]
    fn detects_busy_from_interrupt_hint() {
        let mut detector = StateDetector::new(Tool::Codex);
        let state = detector.detect("Working...\nEsc to interrupt", true);
        assert_eq!(state, Status::Busy);
    }

    #[test]
    fn detects_errors_first() {
        let mut detector = StateDetector::new(Tool::Claude);
        let state = detector.detect("Traceback (most recent call last)", true);
        assert_eq!(state, Status::Error);
    }

    #[test]
    fn detects_claude_ready_from_prompt_glyph_and_shortcuts_hint() {
        let mut detector = StateDetector::new(Tool::Claude);
        let state = detector.detect("❯ \n? for shortcuts", true);
        assert_eq!(state, Status::Ready);
    }

    #[test]
    fn detects_claude_ready_from_confirmation_prompt() {
        let mut detector = StateDetector::new(Tool::Claude);
        let state = detector.detect("Enter to confirm · Esc to cancel", true);
        assert_eq!(state, Status::Ready);
    }

    #[test]
    fn keeps_claude_editor_hint_as_ready_without_interrupt_signal() {
        let mut detector = StateDetector::new(Tool::Claude);
        let state = detector.detect("ctrl+g to edit in VS Code\n● high · /effort", true);
        assert_eq!(state, Status::Ready);
    }

    #[test]
    fn treats_non_empty_startup_screen_as_ready() {
        let mut detector = StateDetector::new(Tool::Claude);
        let state = detector.detect("Starting MCP servers...", true);
        assert_eq!(state, Status::Ready);
    }

    #[test]
    fn keeps_empty_screen_as_starting() {
        let mut detector = StateDetector::new(Tool::Claude);
        let state = detector.detect("", false);
        assert_eq!(state, Status::Starting);
    }
}
