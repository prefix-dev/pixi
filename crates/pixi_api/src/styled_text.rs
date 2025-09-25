pub struct StyledText {
    pub text: String,
    pub bold: bool,
    pub green: bool,
}

impl StyledText {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            bold: false,
            green: false,
        }
    }

    pub fn bold(mut self) -> Self {
        self.bold = true;
        self
    }

    pub fn green(mut self) -> Self {
        self.green = true;
        self
    }

    pub fn text(&self) -> &str {
        &self.text
    }
}

pub trait StyleExt {
    fn style(self) -> StyledText;
}

impl<T: Into<String>> StyleExt for T {
    fn style(self) -> StyledText {
        StyledText::new(self)
    }
}
