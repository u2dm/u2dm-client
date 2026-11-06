use slint_interpreter::{Struct, Value};

use super::values::{flag_of, int_of, strings_of, text_of};

pub(super) fn string_arg(args: &[Value], index: usize) -> String {
    text_of(args.get(index)).to_owned()
}

pub(super) fn bool_arg(args: &[Value], index: usize) -> bool {
    flag_of(args.get(index))
}

pub(super) fn usize_arg(args: &[Value], index: usize) -> Option<usize> {
    match args.get(index) {
        Some(Value::Number(n))
            if n.is_finite() && n.fract() == 0.0 && *n >= 0.0 && *n <= u32::MAX.into() =>
        {
            n.to_string().parse().ok()
        }
        _ => None,
    }
}

pub(super) fn int_arg(args: &[Value], index: usize) -> Option<i32> {
    match args.get(index) {
        Some(Value::Number(n))
            if n.is_finite()
                && n.fract() == 0.0
                && *n >= i32::MIN.into()
                && *n <= i32::MAX.into() =>
        {
            n.to_string().parse().ok()
        }
        _ => None,
    }
}

pub(super) fn struct_arg(args: &[Value], index: usize) -> Option<&Struct> {
    match args.get(index) {
        Some(Value::Struct(s)) => Some(s),
        _ => None,
    }
}

pub(super) fn flag(s: &Struct, name: &str) -> bool {
    flag_of(s.get_field(name))
}

pub(super) fn strings(s: &Struct, name: &str) -> Vec<String> {
    strings_of(s.get_field(name))
        .into_iter()
        .map(String::from)
        .collect()
}

pub(super) fn field(s: &Struct, name: &str) -> String {
    text_of(s.get_field(name)).to_owned()
}

pub(super) fn number(s: &Struct, name: &str) -> i32 {
    int_of(s.get_field(name))
}

pub(super) fn seen_args(args: &[Value]) -> (String, i32, i32, bool) {
    (
        string_arg(args, 0),
        int_arg(args, 1).unwrap_or_default(),
        int_arg(args, 2).unwrap_or_default(),
        bool_arg(args, 3),
    )
}
