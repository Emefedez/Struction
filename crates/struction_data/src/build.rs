//! Turns spanned JSON into reflected values by walking `TypeInfo`, so every mismatch is reported
//! at the value that caused it.

use std::any::TypeId;

use bevy::ecs::reflect::ReflectComponent;
use bevy::reflect::array::DynamicArray;
use bevy::reflect::enums::{DynamicEnum, DynamicVariant, VariantInfo};
use bevy::reflect::list::DynamicList;
use bevy::reflect::map::{DynamicMap, Map};
use bevy::reflect::std_traits::ReflectDefault;
use bevy::reflect::structs::DynamicStruct;
use bevy::reflect::tuple::DynamicTuple;
use bevy::reflect::tuple_struct::DynamicTupleStruct;
use bevy::reflect::{
    PartialReflect, Reflect, ReflectDeserialize, ReflectFromReflect, TypeInfo, TypeRegistration,
    TypeRegistry,
};

use crate::error::{DataError, ErrorKind};
use crate::source::{Node, NodeValue, Span};
use crate::typeinfo::{Primitive, display_name, info_for, is_option, primitive};

/// One component of a resolved definition, ready to insert into an entity.
#[derive(Debug)]
pub struct ComponentValue {
    pub type_id: TypeId,
    pub type_path: &'static str,
    pub value: Box<dyn Reflect>,
}

pub(crate) struct Builder<'r> {
    pub registry: &'r TypeRegistry,
}

type Built = Result<Box<dyn PartialReflect>, DataError>;

fn fail<T>(kind: ErrorKind, span: &Span) -> Result<T, DataError> {
    Err(DataError::at(kind, span))
}

fn mismatch<T>(expected: impl Into<String>, node: &Node) -> Result<T, DataError> {
    fail(
        ErrorKind::TypeMismatch {
            expected: expected.into(),
            found: node.kind_name().into(),
        },
        &node.span,
    )
}

impl Builder<'_> {
    /// Finds the registered component a definition key refers to.
    pub fn component_registration(
        &self,
        name: &str,
        span: &Span,
    ) -> Result<&TypeRegistration, DataError> {
        let registration = if name.contains("::") {
            self.registry.get_with_type_path(name)
        } else if self.registry.is_ambiguous(name) {
            let candidates = self
                .registry
                .iter()
                .filter(|r| r.type_info().type_path_table().short_path() == name)
                .map(|r| r.type_info().type_path().to_owned())
                .collect();
            return fail(
                ErrorKind::AmbiguousComponent {
                    name: name.into(),
                    candidates,
                },
                span,
            );
        } else {
            self.registry.get_with_short_type_path(name)
        };
        let Some(registration) = registration else {
            return fail(ErrorKind::UnknownComponent(name.into()), span);
        };
        if registration.data::<ReflectComponent>().is_none() {
            return fail(ErrorKind::NotAComponent(name.into()), span);
        }
        Ok(registration)
    }

    pub fn component(
        &self,
        registration: &TypeRegistration,
        node: &Node,
    ) -> Result<ComponentValue, DataError> {
        let info = registration.type_info();
        let partial = self.build(node, info)?;
        let value = self.finish(registration, partial, node)?;
        Ok(ComponentValue {
            type_id: registration.type_id(),
            type_path: info.type_path(),
            value,
        })
    }

    /// Completes a possibly partial value: fields left out come from `Default` when the type has
    /// it, otherwise every field must have been given.
    fn finish(
        &self,
        registration: &TypeRegistration,
        partial: Box<dyn PartialReflect>,
        node: &Node,
    ) -> Result<Box<dyn Reflect>, DataError> {
        let info = registration.type_info();
        if let Some(default) = registration.data::<ReflectDefault>() {
            let mut value = default.default();
            value.apply(&*partial);
            return Ok(value);
        }
        if let Some(from_reflect) = registration.data::<ReflectFromReflect>()
            && let Some(value) = from_reflect.from_reflect(&*partial)
        {
            return Ok(value);
        }
        if let (TypeInfo::Struct(si), NodeValue::Object(given)) = (info, &node.value) {
            let fields: Vec<String> = si
                .iter()
                .filter(|f| !given.iter().any(|m| m.key == f.name()))
                .map(|f| f.name().to_owned())
                .collect();
            if !fields.is_empty() {
                return fail(
                    ErrorKind::MissingField {
                        fields,
                        ty: display_name(info).into(),
                    },
                    &node.span,
                );
            }
        }
        fail(
            ErrorKind::InvalidValue {
                ty: display_name(info).into(),
                message: "could not build the value from the given fields".into(),
            },
            &node.span,
        )
    }

    fn info_of(
        &self,
        ty: bevy::reflect::Type,
        own: Option<&'static TypeInfo>,
        span: &Span,
    ) -> Result<&'static TypeInfo, DataError> {
        info_for(self.registry, ty, own)
            .ok_or_else(|| DataError::at(ErrorKind::UnsupportedType(ty.path().into()), span))
    }

    // Collection insertion constructs a new concrete value; Bevy's list/map apply can panic
    // if handed an incomplete dynamic struct. Finish each entry before it reaches apply.
    fn collection_value(&self, node: &Node, info: &'static TypeInfo) -> Built {
        let partial = self.build(node, info)?;
        match self.registry.get(info.type_id()) {
            Some(registration) => self
                .finish(registration, partial, node)
                .map(|v| v.into_partial_reflect()),
            None => Ok(partial),
        }
    }

    fn build(&self, node: &Node, info: &'static TypeInfo) -> Built {
        let name = display_name(info);
        match info {
            TypeInfo::Struct(si) => match &node.value {
                NodeValue::Object(members) => {
                    let mut out = DynamicStruct::default();
                    out.set_represented_type(Some(info));
                    for member in members {
                        let Some(field) = si.field(&member.key) else {
                            return fail(
                                ErrorKind::UnknownField {
                                    field: member.key.clone(),
                                    ty: name.into(),
                                },
                                &member.key_span,
                            );
                        };
                        let field_info =
                            self.info_of(*field.ty(), field.type_info(), &member.value.span)?;
                        out.insert_boxed(field.name(), self.build(&member.value, field_info)?);
                    }
                    Ok(Box::new(out))
                }
                // Types like `Vec3` are structs to reflection but arrays in data.
                NodeValue::Array(_)
                    if self
                        .registry
                        .get_type_data::<ReflectDeserialize>(info.type_id())
                        .is_some() =>
                {
                    self.deserialize(node, info)
                }
                _ => mismatch(format!("object for {name}"), node),
            },
            TypeInfo::TupleStruct(ti) => {
                let mut out = DynamicTupleStruct::default();
                out.set_represented_type(Some(info));
                let items = self.tuple_items(node, ti.field_len(), name)?;
                for (i, item) in items.into_iter().enumerate() {
                    let field = ti.field_at(i).expect("length checked");
                    let field_info = self.info_of(*field.ty(), field.type_info(), &item.span)?;
                    out.insert_boxed(self.build(item, field_info)?);
                }
                Ok(Box::new(out))
            }
            TypeInfo::Tuple(ti) => {
                let mut out = DynamicTuple::default();
                out.set_represented_type(Some(info));
                for (i, item) in self
                    .tuple_items(node, ti.field_len(), name)?
                    .into_iter()
                    .enumerate()
                {
                    let field = ti.field_at(i).expect("length checked");
                    let field_info = self.info_of(*field.ty(), field.type_info(), &item.span)?;
                    out.insert_boxed(self.build(item, field_info)?);
                }
                Ok(Box::new(out))
            }
            TypeInfo::List(li) => {
                let NodeValue::Array(items) = &node.value else {
                    return mismatch(format!("array for {name}"), node);
                };
                let item_info = self.info_of(li.item_ty(), li.item_info(), &node.span)?;
                let mut out = DynamicList::default();
                out.set_represented_type(Some(info));
                for item in items {
                    out.push_box(self.collection_value(item, item_info)?);
                }
                Ok(Box::new(out))
            }
            TypeInfo::Array(ai) => {
                let NodeValue::Array(items) = &node.value else {
                    return mismatch(format!("array for {name}"), node);
                };
                if items.len() != ai.capacity() {
                    return fail(
                        ErrorKind::InvalidValue {
                            ty: name.into(),
                            message: format!(
                                "expected {} elements, found {}",
                                ai.capacity(),
                                items.len()
                            ),
                        },
                        &node.span,
                    );
                }
                let item_info = self.info_of(ai.item_ty(), ai.item_info(), &node.span)?;
                let built = items
                    .iter()
                    .map(|item| self.collection_value(item, item_info))
                    .collect::<Result<Vec<_>, _>>()?;
                let mut out = DynamicArray::new(built.into_boxed_slice());
                out.set_represented_type(Some(info));
                Ok(Box::new(out))
            }
            TypeInfo::Map(mi) => {
                let NodeValue::Object(members) = &node.value else {
                    return mismatch(format!("object for {name}"), node);
                };
                if primitive(mi.key_ty().id()) != Some(Primitive::Str) {
                    return fail(ErrorKind::UnsupportedType(name.into()), &node.span);
                }
                let value_info = self.info_of(mi.value_ty(), mi.value_info(), &node.span)?;
                let mut out = DynamicMap::default();
                out.set_represented_type(Some(info));
                for member in members {
                    out.insert_boxed(
                        Box::new(member.key.clone()),
                        self.collection_value(&member.value, value_info)?,
                    );
                }
                Ok(Box::new(out))
            }
            TypeInfo::Enum(_) => self.build_enum(node, info),
            TypeInfo::Opaque(_) => self.build_opaque(node, info),
            TypeInfo::Set(_) => fail(ErrorKind::UnsupportedType(name.into()), &node.span),
        }
    }

    /// A one-field tuple (struct, variant) is written as its field, always; longer ones are arrays.
    fn tuple_items<'n>(
        &self,
        node: &'n Node,
        len: usize,
        name: &str,
    ) -> Result<Vec<&'n Node>, DataError> {
        if len == 1 {
            return Ok(vec![node]);
        }
        let NodeValue::Array(items) = &node.value else {
            return mismatch(format!("array for {name}"), node);
        };
        if items.len() != len {
            return fail(
                ErrorKind::InvalidValue {
                    ty: name.into(),
                    message: format!("expected {len} elements, found {}", items.len()),
                },
                &node.span,
            );
        }
        Ok(items.iter().collect())
    }

    fn build_enum(&self, node: &Node, info: &'static TypeInfo) -> Built {
        let TypeInfo::Enum(ei) = info else {
            unreachable!("caller matched an enum")
        };
        let name = display_name(info);
        let finish = |variant: &str, payload: DynamicVariant| -> Built {
            let mut out = DynamicEnum::new(variant, payload);
            out.set_represented_type(Some(info));
            Ok(Box::new(out))
        };

        if is_option(info) {
            if node.is_null() {
                return finish("None", DynamicVariant::Unit);
            }
            let Some(VariantInfo::Tuple(some)) = ei.variant("Some") else {
                return fail(ErrorKind::UnsupportedType(name.into()), &node.span);
            };
            let field = some.field_at(0).expect("Some has one field");
            let inner = self.info_of(*field.ty(), field.type_info(), &node.span)?;
            let mut payload = DynamicTuple::default();
            payload.insert_boxed(self.build(node, inner)?);
            return finish("Some", payload.into());
        }

        let unknown = |variant: &str, span: &Span| {
            fail(
                ErrorKind::UnknownVariant {
                    variant: variant.into(),
                    ty: name.into(),
                },
                span,
            )
        };
        match &node.value {
            NodeValue::String(variant) => match ei.variant(variant) {
                Some(VariantInfo::Unit(_)) => finish(variant, DynamicVariant::Unit),
                Some(_) => fail(
                    ErrorKind::InvalidValue {
                        ty: name.into(),
                        message: format!(
                            "variant \"{variant}\" carries data, write {{ \"{variant}\": ... }}"
                        ),
                    },
                    &node.span,
                ),
                None => unknown(variant, &node.span),
            },
            NodeValue::Object(members) if members.len() == 1 => {
                let member = &members[0];
                let payload = &member.value;
                match ei.variant(&member.key) {
                    None => unknown(&member.key, &member.key_span),
                    Some(VariantInfo::Unit(_)) => {
                        if payload.is_null() {
                            finish(&member.key, DynamicVariant::Unit)
                        } else {
                            mismatch("null for a unit variant", payload)
                        }
                    }
                    Some(VariantInfo::Tuple(ti)) => {
                        let mut out = DynamicTuple::default();
                        let items = self.tuple_items(payload, ti.field_len(), &member.key)?;
                        for (i, item) in items.into_iter().enumerate() {
                            let field = ti.field_at(i).expect("length checked");
                            let field_info =
                                self.info_of(*field.ty(), field.type_info(), &item.span)?;
                            out.insert_boxed(self.build(item, field_info)?);
                        }
                        finish(&member.key, out.into())
                    }
                    Some(VariantInfo::Struct(si)) => {
                        let NodeValue::Object(fields) = &payload.value else {
                            return mismatch("object", payload);
                        };
                        let mut out = DynamicStruct::default();
                        for field_member in fields {
                            let Some(field) = si.field(&field_member.key) else {
                                return fail(
                                    ErrorKind::UnknownField {
                                        field: field_member.key.clone(),
                                        ty: format!("{name}::{}", member.key),
                                    },
                                    &field_member.key_span,
                                );
                            };
                            let field_info = self.info_of(
                                *field.ty(),
                                field.type_info(),
                                &field_member.value.span,
                            )?;
                            out.insert_boxed(
                                field.name(),
                                self.build(&field_member.value, field_info)?,
                            );
                        }
                        // Reflection cannot fill a variant partially, so require every field
                        // but the optional ones, which are `None` when left out.
                        let mut missing = Vec::new();
                        for field in si.iter() {
                            if fields.iter().any(|m| m.key == field.name()) {
                                continue;
                            }
                            let field_info =
                                self.info_of(*field.ty(), field.type_info(), &payload.span)?;
                            if is_option(field_info) {
                                let none = Node {
                                    span: payload.span.clone(),
                                    value: NodeValue::Null,
                                };
                                out.insert_boxed(field.name(), self.build(&none, field_info)?);
                            } else {
                                missing.push(field.name().to_owned());
                            }
                        }
                        if !missing.is_empty() {
                            return fail(
                                ErrorKind::MissingField {
                                    fields: missing,
                                    ty: format!("{name}::{}", member.key),
                                },
                                &payload.span,
                            );
                        }
                        finish(&member.key, out.into())
                    }
                }
            }
            _ => mismatch(
                format!("variant name or {{ \"Variant\": ... }} for {name}"),
                node,
            ),
        }
    }

    fn deserialize(&self, node: &Node, info: &'static TypeInfo) -> Built {
        let name = display_name(info);
        let Some(de) = self
            .registry
            .get_type_data::<ReflectDeserialize>(info.type_id())
        else {
            return fail(ErrorKind::UnsupportedType(name.into()), &node.span);
        };
        match de.deserialize(node.to_value()) {
            Ok(value) => Ok(value.into_partial_reflect()),
            Err(e) => fail(
                ErrorKind::InvalidValue {
                    ty: name.into(),
                    message: e.to_string(),
                },
                &node.span,
            ),
        }
    }

    fn build_opaque(&self, node: &Node, info: &'static TypeInfo) -> Built {
        let name = display_name(info);
        let Some(kind) = primitive(info.type_id()) else {
            return self.deserialize(node, info);
        };
        let id = info.type_id();
        let out_of_range = |text: String| {
            fail(
                ErrorKind::InvalidValue {
                    ty: name.into(),
                    message: format!("{text} is out of range"),
                },
                &node.span,
            )
        };
        match (kind, &node.value) {
            (Primitive::Bool, NodeValue::Bool(b)) => Ok(Box::new(*b)),
            (Primitive::Str, NodeValue::String(s)) => Ok(Box::new(s.clone())),
            (Primitive::Char, NodeValue::String(s)) => {
                let mut chars = s.chars();
                match (chars.next(), chars.next()) {
                    (Some(c), None) => Ok(Box::new(c)),
                    _ => mismatch("single-character string", node),
                }
            }
            (Primitive::Float, NodeValue::Number(n)) => {
                let v = n.as_f64().expect("json numbers are finite");
                if id == TypeId::of::<f32>() {
                    Ok(Box::new(v as f32))
                } else {
                    Ok(Box::new(v))
                }
            }
            (Primitive::Signed, NodeValue::Number(n)) => {
                let Some(v) = n.as_i64() else {
                    return if n.is_u64() {
                        out_of_range(n.to_string())
                    } else {
                        mismatch(format!("integer ({name})"), node)
                    };
                };
                macro_rules! narrow {
                    ($($ty:ty),+) => {$(
                        if id == TypeId::of::<$ty>() {
                            return match <$ty>::try_from(v) {
                                Ok(v) => Ok(Box::new(v)),
                                Err(_) => out_of_range(v.to_string()),
                            };
                        }
                    )+};
                }
                narrow!(i8, i16, i32, i64, i128, isize);
                unreachable!("classified as signed")
            }
            (Primitive::Unsigned, NodeValue::Number(n)) => {
                let Some(v) = n.as_u64() else {
                    return if n.as_i64().is_some() {
                        out_of_range(n.to_string())
                    } else {
                        mismatch(format!("non-negative integer ({name})"), node)
                    };
                };
                macro_rules! narrow {
                    ($($ty:ty),+) => {$(
                        if id == TypeId::of::<$ty>() {
                            return match <$ty>::try_from(v) {
                                Ok(v) => Ok(Box::new(v)),
                                Err(_) => out_of_range(v.to_string()),
                            };
                        }
                    )+};
                }
                narrow!(u8, u16, u32, u64, u128, usize);
                unreachable!("classified as unsigned")
            }
            _ => mismatch(name, node),
        }
    }
}
