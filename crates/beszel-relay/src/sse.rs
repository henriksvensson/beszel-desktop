//! Incremental Server-Sent Events (SSE) parser for PocketBase `/api/realtime`.

/// A single parsed SSE event frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseEvent {
    pub id: Option<String>,
    pub event: Option<String>,
    pub data: String,
}

/// Stateful line-buffered SSE frame decoder.
#[derive(Debug, Default)]
pub struct SseParser {
    buffer: String,
    current_id: Option<String>,
    current_event: Option<String>,
    current_data: Vec<String>,
}

impl SseParser {
    /// Feed a raw UTF-8 chunk from the HTTP stream and collect any completed frames.
    pub fn push_chunk(&mut self, chunk: &[u8]) -> Vec<SseEvent> {
        self.buffer.push_str(&String::from_utf8_lossy(chunk));
        let mut events = Vec::new();

        while let Some(newline_pos) = self.buffer.find('\n') {
            let mut line = self.buffer[..newline_pos].to_string();
            self.buffer.drain(..=newline_pos);
            if line.ends_with('\r') {
                line.pop();
            }
            if let Some(event) = self.process_line(&line) {
                events.push(event);
            }
        }
        events
    }

    fn process_line(&mut self, line: &str) -> Option<SseEvent> {
        if line.is_empty() {
            return self.flush_event();
        }
        if line.starts_with(':') {
            return None;
        }
        let (field, value) = split_field(line);
        match field {
            "event" => self.current_event = Some(value.to_string()),
            "id" => self.current_id = Some(value.to_string()),
            "data" => self.current_data.push(value.to_string()),
            _ => {}
        }
        None
    }

    fn flush_event(&mut self) -> Option<SseEvent> {
        if self.current_event.is_none() && self.current_data.is_empty() {
            self.current_id = None;
            return None;
        }
        let event = SseEvent {
            id: self.current_id.take(),
            event: self.current_event.take(),
            data: self.current_data.join("\n"),
        };
        self.current_data.clear();
        Some(event)
    }
}

fn split_field(line: &str) -> (&str, &str) {
    match line.split_once(':') {
        Some((field, rest)) => (field, rest.strip_prefix(' ').unwrap_or(rest)),
        None => (line, ""),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_pb_connect_and_rt_metrics_across_split_chunks() {
        let mut parser = SseParser::default();
        let part1 = b": keepalive\r\nid: 1\r\nevent: PB_CONNECT\r\ndata: {\"client";
        let part2 = b"Id\":\"abc123\"}\r\n\r\nevent: rt_metrics\ndata: {\"stats\":{\"cpu\":5}}\n\n";

        assert!(parser.push_chunk(part1).is_empty());
        let events = parser.push_chunk(part2);
        assert_eq!(events.len(), 2);

        assert_eq!(
            events[0],
            SseEvent {
                id: Some("1".into()),
                event: Some("PB_CONNECT".into()),
                data: "{\"clientId\":\"abc123\"}".into(),
            }
        );
        assert_eq!(
            events[1],
            SseEvent {
                id: None,
                event: Some("rt_metrics".into()),
                data: "{\"stats\":{\"cpu\":5}}".into(),
            }
        );
    }

    #[test]
    fn joins_multiline_data_fields() {
        let mut parser = SseParser::default();
        let events = parser.push_chunk(b"event: test\ndata: line1\ndata: line2\n\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "line1\nline2");
    }
}
