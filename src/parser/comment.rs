/// A comment's textual content. The source line is carried by the
/// enclosing `Located<Comment>` wrapper.
#[derive(Debug, Clone)]
pub struct Comment {
    pub text: String,
}
