use prjunnamed_netlist::{Net, Value};
use std::fmt;

#[derive(Debug)]
pub enum Expression<'a> {
    NamedValue(&'a str),
    PendingNet(Net),
    Concat(Vec<Expression<'a>>),
}

pub use Expression::{NamedValue, PendingNet, Concat};

impl<'a> Expression<'a> {
    pub fn from_value<'x, 'y>(value: &'x Value) -> Expression<'y> {
        if value.is_empty() {
            panic!("attempt to create a zero-length expression");
        }

        if value.len() == 1 {
            PendingNet(value.unwrap_net())
        } else {
            Concat(value.iter().rev().map(|net| PendingNet(net)).collect::<Vec<Expression>>())
        }
    }

    pub fn visit_expressions(&mut self, f: &mut impl FnMut(&mut Expression<'a>)) {
        match self {
            Concat(vec) => {
                for expr in vec.iter_mut() {
                    f(expr)
                }
            }
            _ => {}
        }
        f(self);
    }
}

pub type Type<'a> = &'a str;
pub type Name<'a> = &'a str;
pub struct Attrs<'a>(pub Vec<(&'a str, &'a str)>);
pub type Connections<'a> = Vec<(Name<'a>, Expression<'a>)>;

#[derive(Clone, Copy)]
pub struct Range {
    pub msb: usize,
    pub lsb: usize,
}

impl Range {
    pub fn len(self) -> usize {
        if self.msb >= self.lsb { self.msb - self.lsb + 1 } else { self.lsb - self.msb + 1 }
    }
}

pub enum Member<'a> {
    Output(Attrs<'a>, Name<'a>, Range),
    Input(Attrs<'a>, Name<'a>, Range),
    Wire(Attrs<'a>, Name<'a>, Range),
    Assign(Expression<'a>, Expression<'a>),
    // TODO: add parameters
    Instantiation(Attrs<'a>, Type<'a>, Name<'a>, Connections<'a>),
}

pub use Member::{Input, Output, Wire, Assign, Instantiation};

impl<'a> Member<'a> {
    pub fn visit_expressions(&mut self, f: &mut impl FnMut(&mut Expression<'a>)) {
        match self {
            Member::Instantiation(_, _, _, conns) => {
                for (_, expr) in conns.iter_mut() {
                    expr.visit_expressions(f);
                }
            }
            Member::Assign(lhs, rhs) => {
                lhs.visit_expressions(f);
                rhs.visit_expressions(f);
            }
            _ => {}
        }
    }
}

pub struct Module<'a> {
    pub name: &'a str,
    pub port_members: Vec<Member<'a>>,
    pub members: Vec<Member<'a>>,
}

pub struct Unit<'a>(pub Vec<Module<'a>>);

impl<'a> fmt::Display for Unit<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for module in &self.0 {
            write!(f, "{}", module)?;
        }
        Ok(())
    }
}

impl<'a> fmt::Display for Module<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // TODO: proper escaping of an identifier
        write!(f, "module \\{} (", self.name)?;
        let mut first = true;
        for member in &self.port_members {
            match member {
                Member::Input(_, name, _) | Member::Output(_, name, _) => {
                    if !first {
                        write!(f, ",")?;
                    }
                    write!(f, "\n  \\{} ", name)?;
                }
                _ => {
                    unreachable!();
                }
            }
            first = false;
        }
        write!(f, "\n);\n")?;
        for member in &self.port_members {
            write!(f, "  {};\n", member)?;
        }
        for member in &self.members {
            write!(f, "  {};\n", member)?;
        }
        write!(f, "endmodule\n\n")?;
        Ok(())
    }
}

impl<'a> fmt::Display for Attrs<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (a, b) in &self.0 {
            // TODO: escaping
            writeln!(f, "(* {} = \"{}\" *)", a, b)?;
        }
        Ok(())
    }
}

impl<'a> fmt::Display for Member<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Input(attrs, name, range) => {
                write!(f, "{}input{} \\{} ", attrs, range, name)?;
            }
            Output(attrs, name, range) => {
                write!(f, "{}output{} \\{} ", attrs, range, name)?;
            }
            Wire(attrs, name, range) => {
                write!(f, "{}wire{} \\{} ", attrs, range, name)?;
            }
            Assign(lhs, rhs) => {
                write!(f, "assign {} = {}", lhs, rhs)?;
            }
            Instantiation(attrs, type_, name, conns) => {
                write!(f, "{}\\{} \\{} (", attrs, type_, name)?;
                let mut first = true;
                for (port_name, expr) in conns.iter() {
                    if !first {
                        write!(f, ",")?;
                    }
                    write!(f, "\n    .\\{} ({})", port_name, expr)?;
                    first = false;
                }
                write!(f, "\n  )")?;
            }
        }
        Ok(())
    }
}

impl<'a> fmt::Display for Expression<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NamedValue(name) => {
                write!(f, "\\{} ", name)?;
            }
            Concat(vec) => {
                write!(f, "{{")?;
                let mut first = true;
                for expr in vec {
                    if !first {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", expr)?;
                    first = false;
                }
                write!(f, "}}")?;
            }
            _ => {
                unimplemented!("net {:?}", self);
            }
        }
        Ok(())
    }
}

impl<'a> fmt::Display for Range {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.msb != 0 || self.lsb != 0 {
            write!(f, " [{}:{}]", self.msb, self.lsb)?;
        }
        Ok(())
    }
}
