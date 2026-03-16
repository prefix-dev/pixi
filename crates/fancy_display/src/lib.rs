pub trait FancyDisplay {
    fn fancy_display(&self) -> console::StyledObject<&str>;
}
