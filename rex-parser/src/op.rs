use std::fmt::{self, Display, Formatter};

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum Operator {
    Add,
    And,
    Concat,
    Div,
    Dot,
    Eq,
    Ne,
    Ge,
    Gt,
    Le,
    Lt,
    Mod,
    Mul,
    Or,
    Sub,
}

impl Display for Operator {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let s = match self {
            Operator::Add => "+".to_string(),
            Operator::And => "&&".to_string(),
            Operator::Concat => "++".to_string(),
            Operator::Div => "/".to_string(),
            Operator::Dot => ".".to_string(),
            Operator::Eq => "==".to_string(),
            Operator::Ne => "!=".to_string(),
            Operator::Ge => ">=".to_string(),
            Operator::Gt => ">".to_string(),
            Operator::Le => "<=".to_string(),
            Operator::Lt => "<".to_string(),
            Operator::Mod => "%".to_string(),
            Operator::Mul => "*".to_string(),
            Operator::Or => "||".to_string(),
            Operator::Sub => "-".to_string(),
        };
        write!(f, "{}", s)
    }
}
