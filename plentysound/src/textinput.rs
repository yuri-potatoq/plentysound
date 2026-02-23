pub struct TextInput {
    buf: String,
    cursor: usize,
}

impl TextInput {
    pub fn new() -> Self {
        TextInput {
            buf: String::new(),
            cursor: 0,
        }
    }

    pub fn push_char(&mut self, c: char) {
        self.buf.insert(self.cursor, c);
        self.cursor += c.len_utf8();
    }

    pub fn backspace(&mut self) {
        if self.cursor > 0 {
            let prev = self.buf[..self.cursor]
                .char_indices()
                .next_back()
                .map(|(i, _)| i)
                .unwrap_or(0);
            self.buf.drain(prev..self.cursor);
            self.cursor = prev;
        }
    }

    pub fn clear(&mut self) {
        self.buf.clear();
        self.cursor = 0;
    }

    pub fn as_str(&self) -> &str {
        &self.buf
    }

    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    pub fn cursor_pos(&self) -> usize {
        self.buf[..self.cursor].chars().count()
    }
}
