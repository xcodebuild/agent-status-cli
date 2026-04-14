#[derive(Clone, Debug, Default)]
pub struct FilterOutput {
    pub passthrough: Vec<u8>,
    pub title: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum State {
    Ground,
    Escape,
    Osc,
    OscEscape,
}

pub struct OscFilter {
    state: State,
    osc_buffer: Vec<u8>,
}

impl Default for OscFilter {
    fn default() -> Self {
        Self {
            state: State::Ground,
            osc_buffer: Vec::new(),
        }
    }
}

impl OscFilter {
    pub fn feed(&mut self, bytes: &[u8]) -> FilterOutput {
        let mut output = FilterOutput::default();

        for &byte in bytes {
            match self.state {
                State::Ground => {
                    if byte == 0x1b {
                        self.state = State::Escape;
                    } else {
                        output.passthrough.push(byte);
                    }
                }
                State::Escape => {
                    if byte == b']' {
                        self.osc_buffer.clear();
                        self.osc_buffer.extend_from_slice(&[0x1b, b']']);
                        self.state = State::Osc;
                    } else {
                        output.passthrough.push(0x1b);
                        output.passthrough.push(byte);
                        self.state = State::Ground;
                    }
                }
                State::Osc => {
                    self.osc_buffer.push(byte);
                    if byte == 0x07 {
                        self.finish_osc(&mut output);
                    } else if byte == 0x1b {
                        self.state = State::OscEscape;
                    }
                }
                State::OscEscape => {
                    self.osc_buffer.push(byte);
                    if byte == b'\\' {
                        self.finish_osc(&mut output);
                    } else {
                        self.state = State::Osc;
                    }
                }
            }
        }

        output
    }

    fn finish_osc(&mut self, output: &mut FilterOutput) {
        let osc = std::mem::take(&mut self.osc_buffer);
        match parse_osc_title(&osc) {
            Some(title) => output.title = Some(title),
            None => output.passthrough.extend_from_slice(&osc),
        }
        self.state = State::Ground;
    }
}

fn parse_osc_title(bytes: &[u8]) -> Option<String> {
    if !bytes.starts_with(&[0x1b, b']']) {
        return None;
    }

    let payload = if bytes.ends_with(&[0x07]) {
        &bytes[2..bytes.len() - 1]
    } else if bytes.ends_with(&[0x1b, b'\\']) {
        &bytes[2..bytes.len() - 2]
    } else {
        return None;
    };

    let text = String::from_utf8_lossy(payload);
    let (kind, value) = text.split_once(';')?;
    match kind {
        "0" | "1" | "2" => Some(value.to_owned()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_title_osc_and_keeps_other_bytes() {
        let mut filter = OscFilter::default();
        let output = filter.feed(b"hello\x1b]0;Codex title\x07world");

        assert_eq!(String::from_utf8(output.passthrough).unwrap(), "helloworld");
        assert_eq!(output.title.as_deref(), Some("Codex title"));
    }

    #[test]
    fn keeps_non_title_osc_sequences() {
        let mut filter = OscFilter::default();
        let output = filter.feed(b"\x1b]1337;SetProfile=Demo\x07");

        assert_eq!(
            String::from_utf8(output.passthrough).unwrap(),
            "\u{1b}]1337;SetProfile=Demo\u{7}"
        );
        assert_eq!(output.title, None);
    }

    #[test]
    fn handles_split_osc_sequences() {
        let mut filter = OscFilter::default();
        let first = filter.feed(b"\x1b]2;Par");
        let second = filter.feed(b"tial\x1b\\done");

        assert_eq!(first.passthrough, b"");
        assert_eq!(second.title.as_deref(), Some("Partial"));
        assert_eq!(String::from_utf8(second.passthrough).unwrap(), "done");
    }
}
