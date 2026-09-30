//! Sections that become core components: `reactions` ([`Reactions`]) and `grantsToWards`
//! ([`GrantsToWards`]). Action names are resolved against the [`ActionRegistry`], so these run
//! after every package registered its actions; errors point at the offending value.
//!
//! ```jsonc
//! "reactions": [
//!   // `source` is "this" (default), "master" or "wards"; exactly one of `after` / `before`.
//!   { "source": "master", "after": "bosses/ogre_lord/die", "call": "minions/ogre/die", "args": {} }
//! ],
//! "grantsToWards": [
//!   { "to": "minions/ogre", "components": { "Follower": { "distance": 3 } }, "actions": ["fetch"] }
//! ]
//! ```

use bevy::reflect::TypeRegistry;
use struction_core::{
    ActionArgs, ActionError, ActionName, ActionRegistry, ArgValue, GrantRule, GrantsToWards,
    Reaction, ReactionSource, Reactions,
};

use crate::definition::Resolved;
use crate::error::{DataError, ErrorKind};
use crate::source::{Member, Node, NodeValue};
use crate::store::{DefinitionStore, build_component_map};

fn mismatch(expected: &str, node: &Node) -> DataError {
    DataError::at(
        ErrorKind::TypeMismatch {
            expected: expected.into(),
            found: node.kind_name().into(),
        },
        &node.span,
    )
}

fn string(node: &Node) -> Result<&str, DataError> {
    node.as_str().ok_or_else(|| mismatch("string", node))
}

fn object<'n>(node: &'n Node, what: &str) -> Result<&'n [Member], DataError> {
    node.as_object().ok_or_else(|| mismatch(what, node))
}

fn array<'n>(node: &'n Node, what: &str) -> Result<&'n [Node], DataError> {
    node.as_array().ok_or_else(|| mismatch(what, node))
}

fn action_error(error: ActionError, node: &Node) -> DataError {
    DataError::at(ErrorKind::Action(error.to_string()), &node.span)
}

fn arg_value(node: &Node) -> Result<ArgValue, DataError> {
    match &node.value {
        NodeValue::Bool(b) => Ok(ArgValue::Bool(*b)),
        NodeValue::String(s) => Ok(ArgValue::Str(s.clone())),
        NodeValue::Number(n) => Ok(match n.as_i64() {
            Some(i) => ArgValue::Int(i),
            None => ArgValue::Float(n.as_f64().unwrap_or(f64::NAN)),
        }),
        _ => Err(mismatch("boolean, number or string argument", node)),
    }
}

fn reaction(node: &Node, actions: &ActionRegistry) -> Result<Reaction, DataError> {
    let mut source = ReactionSource::This;
    let mut hook = None;
    let mut call = None;
    let mut args = ActionArgs::new();
    for member in object(node, "reaction object")? {
        let value = &member.value;
        match member.key.as_str() {
            "source" => {
                source = match string(value)? {
                    "this" => ReactionSource::This,
                    "master" => ReactionSource::Master,
                    "wards" => ReactionSource::Wards,
                    other => {
                        return Err(DataError::at(
                            ErrorKind::UnknownVariant {
                                variant: other.into(),
                                ty: "reaction source (this, master, wards)".into(),
                            },
                            &value.span,
                        ));
                    }
                }
            }
            "after" | "before" => {
                if hook.is_some() {
                    return Err(DataError::at(
                        ErrorKind::InvalidValue {
                            ty: "reaction".into(),
                            message: "give only one of \"after\" and \"before\"".into(),
                        },
                        &member.key_span,
                    ));
                }
                hook = Some((member.key == "after", value));
            }
            "call" => call = Some(value),
            "args" => {
                for arg in object(value, "object of arguments")? {
                    args = args.with(arg.key.clone(), arg_value(&arg.value)?);
                }
            }
            _ => {
                return Err(DataError::at(
                    ErrorKind::UnknownField {
                        field: member.key.clone(),
                        ty: "reaction".into(),
                    },
                    &member.key_span,
                ));
            }
        }
    }
    let missing: Vec<String> = [
        hook.is_none().then(|| "after (or before)".to_owned()),
        call.is_none().then(|| "call".to_owned()),
    ]
    .into_iter()
    .flatten()
    .collect();
    let (Some((after, on_node)), Some(call_node)) = (hook, call) else {
        return Err(DataError::at(
            ErrorKind::MissingField {
                fields: missing,
                ty: "reaction".into(),
            },
            &node.span,
        ));
    };
    let on = string(on_node)?;
    let call = string(call_node)?;
    let reaction = if after {
        Reaction::after(source, on, call)
    } else {
        Reaction::before(source, on, call)
    }
    .with_args(args);
    reaction.validate(actions).map_err(|error| {
        let at = match &error {
            ActionError::Unknown { name } if name.as_str() == on => on_node,
            _ => call_node,
        };
        action_error(error, at)
    })?;
    Ok(reaction)
}

/// Builds the core [`Reactions`] from a `reactions` array, validating every reaction.
pub fn reactions_from_node(
    node: &Node,
    actions: &ActionRegistry,
) -> Result<Reactions, Vec<DataError>> {
    let items = array(node, "array of reactions").map_err(|e| vec![e])?;
    let mut reactions = Vec::new();
    let mut errors = Vec::new();
    for item in items {
        match reaction(item, actions) {
            Ok(r) => reactions.push(r),
            Err(e) => errors.push(e),
        }
    }
    if errors.is_empty() {
        Ok(Reactions(reactions))
    } else {
        Err(errors)
    }
}

fn grant_rule(
    node: &Node,
    types: &TypeRegistry,
    actions: &ActionRegistry,
    errors: &mut Vec<DataError>,
) -> Option<GrantRule> {
    let members = match object(node, "grant object") {
        Ok(members) => members,
        Err(e) => {
            errors.push(e);
            return None;
        }
    };
    let mut rule: Option<GrantRule> = None;
    let mut components = Vec::new();
    let mut granted = Vec::new();
    let before = errors.len();
    for member in members {
        let value = &member.value;
        let result = match member.key.as_str() {
            "to" => string(value).map(|to| rule = Some(GrantRule::new(to))),
            "components" => match build_component_map(value, types) {
                Ok(built) => {
                    components = built;
                    Ok(())
                }
                Err(e) => {
                    errors.extend(e);
                    Ok(())
                }
            },
            "actions" => array(value, "array of action names").map(|items| {
                for item in items {
                    match string(item).and_then(|name| {
                        let name = ActionName::new(name);
                        actions
                            .resolve(&name)
                            .map(|_| name)
                            .map_err(|e| action_error(e, item))
                    }) {
                        Ok(name) => granted.push(name),
                        Err(e) => errors.push(e),
                    }
                }
            }),
            _ => Err(DataError::at(
                ErrorKind::UnknownField {
                    field: member.key.clone(),
                    ty: "grant".into(),
                },
                &member.key_span,
            )),
        };
        if let Err(e) = result {
            errors.push(e);
        }
    }
    if rule.is_none() && errors.len() == before {
        errors.push(DataError::at(
            ErrorKind::MissingField {
                fields: vec!["to".into()],
                ty: "grant".into(),
            },
            &node.span,
        ));
    }
    let mut rule = rule?;
    rule.components = components
        .into_iter()
        .map(|c| c.value.into_partial_reflect())
        .collect();
    rule.actions = granted;
    Some(rule)
}

/// Builds the core [`GrantsToWards`] from a `grantsToWards` array: components through `Reflect`,
/// action names resolved. Whether `to` names an existing definition is checked by
/// [`DefinitionStore::check_references`], which knows the definitions.
pub fn grants_from_node(
    node: &Node,
    types: &TypeRegistry,
    actions: &ActionRegistry,
) -> Result<GrantsToWards, Vec<DataError>> {
    let items = array(node, "array of grants").map_err(|e| vec![e])?;
    let mut errors = Vec::new();
    let rules: Vec<_> = items
        .iter()
        .filter_map(|item| grant_rule(item, types, actions, &mut errors))
        .collect();
    if errors.is_empty() {
        Ok(GrantsToWards(rules))
    } else {
        Err(errors)
    }
}

impl Resolved {
    /// The definition's reactions, `None` when it declares none.
    pub fn reactions(&self, actions: &ActionRegistry) -> Result<Option<Reactions>, Vec<DataError>> {
        self.section("reactions")
            .map(|node| reactions_from_node(node, actions))
            .transpose()
    }

    /// The definition's grants to its wards, `None` when it declares none.
    pub fn grants_to_wards(
        &self,
        types: &TypeRegistry,
        actions: &ActionRegistry,
    ) -> Result<Option<GrantsToWards>, Vec<DataError>> {
        self.section("grantsToWards")
            .map(|node| grants_from_node(node, types, actions))
            .transpose()
    }
}

impl DefinitionStore {
    /// Checks the action references of every resolved definition (reactions and grants) and that
    /// grant targets exist. Run once actions are registered; sorted, without repeats (a bad
    /// ancestor would otherwise be reported once per descendant).
    pub fn check_references(
        &self,
        types: &TypeRegistry,
        actions: &ActionRegistry,
    ) -> Vec<DataError> {
        let mut errors = Vec::new();
        for id in self.definitions() {
            let resolved = self.get(id).expect("listed definitions resolve");
            if let Err(e) = resolved.reactions(actions) {
                errors.extend(e);
            }
            if let Err(e) = resolved.grants_to_wards(types, actions) {
                errors.extend(e);
            }
            let targets = resolved
                .section("grantsToWards")
                .and_then(Node::as_array)
                .unwrap_or_default();
            for to in targets.iter().filter_map(|grant| grant.get("to")) {
                if let Some(path) = to.as_str()
                    && self.get(path).is_none()
                {
                    errors.push(DataError::at(
                        ErrorKind::MissingReference {
                            field: "to".into(),
                            path: path.into(),
                        },
                        &to.span,
                    ));
                }
            }
        }
        errors.sort_by_key(|e| e.to_string());
        errors.dedup();
        errors
    }
}
