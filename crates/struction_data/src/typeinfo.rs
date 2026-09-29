//! Helpers over `TypeInfo` shared by the reflect builder and the schema generator.

use std::any::TypeId;

use bevy::reflect::{Type, TypeInfo, TypeRegistry};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Primitive {
    Bool,
    Signed,
    Unsigned,
    Float,
    Str,
    Char,
}

pub(crate) fn primitive(id: TypeId) -> Option<Primitive> {
    macro_rules! table {
        ($($kind:ident: $($ty:ty),+;)+) => {
            $($(if id == TypeId::of::<$ty>() { return Some(Primitive::$kind); })+)+
        };
    }
    table! {
        Bool: bool;
        Signed: i8, i16, i32, i64, i128, isize;
        Unsigned: u8, u16, u32, u64, u128, usize;
        Float: f32, f64;
        Str: String;
        Char: char;
    }
    None
}

/// Vector-like opaque types serialized as a fixed-length array of numbers.
pub(crate) fn vector_len(ident: &str) -> Option<usize> {
    if matches!(ident, "Quat" | "DQuat") {
        return Some(4);
    }
    let dims = ident.chars().last()?.to_digit(10)? as usize;
    let base = &ident[..ident.len() - 1];
    let is_vec = matches!(base, "Vec" | "DVec" | "UVec" | "IVec" | "U64Vec" | "I64Vec");
    (is_vec && (2..=4).contains(&dims)).then_some(dims)
}

pub(crate) fn is_option(info: &TypeInfo) -> bool {
    matches!(info, TypeInfo::Enum(_)) && info.type_path().starts_with("core::option::Option<")
}

/// Finds the type info for a field or element: the container's own record when it has one,
/// otherwise the registry's.
pub(crate) fn info_for(
    registry: &TypeRegistry,
    ty: Type,
    own: Option<&'static TypeInfo>,
) -> Option<&'static TypeInfo> {
    own.or_else(|| registry.get_type_info(ty.id()))
}

/// The name users write: the short path, e.g. `Health` for `my_game::Health`.
pub(crate) fn display_name(info: &TypeInfo) -> &'static str {
    info.type_path_table().short_path()
}
