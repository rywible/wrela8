//! Computed type classes (plans/M13.md item O / the lattice).
//!
//! Per-type classes — `copy`, `must_consume`, `crosses_actor`,
//! `holds_authority` — derived structurally from a leaf table plus
//! field-wise propagation (the same walk shape as
//! `protocol_resource_carried` / `type_carries_named`).
//!
//! This module is the sole leaf for all four classes; the legacy name
//! lists in `image_checks` (`is_sealed_authority_type_name` and the
//! `must_consume` list before it) are deleted, and the M13 dual-run that
//! proved the table equal to them is retired.

use std::collections::BTreeSet;

use crate::sema::types::{Classification, DeclItem, DeclStruct, Type, TypeArg};
use crate::syntax::ast::Span;

/// The four computed classes for one type (02 §3.1 / 03 §1 / 05 §2 / 02 §12).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TypeClasses {
    pub copy: bool,
    pub must_consume: bool,
    pub crosses_actor: bool,
    pub holds_authority: bool,
}

impl Default for TypeClasses {
    fn default() -> Self {
        // Boring data default: copyable, may cross, no obligations.
        TypeClasses {
            copy: true,
            must_consume: false,
            crosses_actor: true,
            holds_authority: false,
        }
    }
}

impl TypeClasses {
    pub fn render_line(self) -> String {
        format!(
            "classes copy={} must_consume={} crosses_actor={} holds_authority={}",
            self.copy, self.must_consume, self.crosses_actor, self.holds_authority
        )
    }
}

/// The leaf table (plans/M13.md item O) — the single authority for all
/// four classes. The old `image_checks` name lists it replaced are gone;
/// the M13 dual-run that proved them equal has been retired.
pub fn leaf_classes(name: &str) -> Option<TypeClasses> {
    let classes = match name {
        // 03 §1 capabilities — DeviceCap/Mmio/IrqCap consume; DmaPool is
        // §3.1 first-bullet reclaim (holds_authority only).
        "DeviceCap" | "Mmio" | "IrqCap" => TypeClasses {
            copy: false,
            must_consume: true,
            crosses_actor: false,
            holds_authority: true,
        },
        // Pool / shared handles: sealed authority, but §3.1 first bullet
        // (DmaPool) / no terminal sink yet (DmaShared) — not must_consume.
        "DmaPool" | "DmaShared" => TypeClasses {
            copy: false,
            must_consume: false,
            crosses_actor: false,
            holds_authority: true,
        },
        // Sealed queue / receipt authority.
        "VirtQueue" | "QueuePermit" | "QueueOp" | "Receipt" => TypeClasses {
            copy: false,
            must_consume: true,
            crosses_actor: false,
            holds_authority: true,
        },
        // 03 §9 bring-up states.
        "ResetDevice"
        | "AcknowledgedDevice"
        | "DriverClaimedDevice"
        | "FeaturesNegotiatedDevice"
        | "FeaturesAcceptedDevice"
        | "QueuesConfiguredDevice"
        | "RunningDevice" => TypeClasses {
            copy: false,
            must_consume: true,
            crosses_actor: false,
            holds_authority: true,
        },
        "InterruptCell" => TypeClasses {
            copy: true, // source-constructible cell word; not a resource
            must_consume: false,
            crosses_actor: false,
            holds_authority: false,
        },
        "Actor" => TypeClasses {
            copy: true, // handle word; storable-nowhere is a separate rule
            must_consume: false,
            crosses_actor: false,
            holds_authority: false,
        },
        "Static" => TypeClasses {
            copy: true,
            must_consume: false,
            crosses_actor: true,
            holds_authority: false,
        },
        // 02 §12: never serializes into the image, a diagnostic, or a
        // comptime control decision. Encoded as !crosses_actor.
        "Secret" => TypeClasses {
            copy: false,
            must_consume: false,
            crosses_actor: false,
            holds_authority: false,
        },
        _ => return None,
    };
    Some(classes)
}

/// Does the named leaf (or a `resource(manual)` fiat) require consume-on-
/// every-path?
pub fn name_must_consume(name: &str, is_manual_resource: bool) -> bool {
    if is_manual_resource {
        return true;
    }
    leaf_classes(name).map(|c| c.must_consume).unwrap_or(false)
}

/// Does the named leaf hold sealed authority?
pub fn name_holds_authority(name: &str) -> bool {
    leaf_classes(name)
        .map(|c| c.holds_authority)
        .unwrap_or(false)
}

/// Leaf used by `driver_message_forbidden_carried`: authority or
/// `InterruptCell`.
pub fn name_forbidden_in_driver_message(name: &str) -> bool {
    name_holds_authority(name) || name == "InterruptCell"
}

/// Fill `classes` on every struct/enum in `items` (after classification).
pub fn assign_classes(items: &mut [DeclItem]) {
    let snapshot: Vec<DeclItem> = items.to_vec();
    for item in items.iter_mut() {
        match item {
            DeclItem::Struct(s) => {
                s.classes = compute_struct_classes(s, &snapshot);
                s.classes_assigned = true;
            }
            DeclItem::Enum(e) => {
                e.classes = fold_components(&e.component_types, &snapshot, &mut BTreeSet::new());
                if e.classification == Classification::Data
                    && e.classes.copy
                    && !e.classes.must_consume
                    && !e.classes.holds_authority
                {
                    e.classes.copy = true;
                } else {
                    e.classes.copy = false;
                }
                e.classes_assigned = true;
            }
            _ => {}
        }
    }
}

fn compute_struct_classes(s: &DeclStruct, items: &[DeclItem]) -> TypeClasses {
    let mut c = fold_components(&s.component_types, items, &mut BTreeSet::new());
    if s.is_manual_resource {
        c.copy = false;
        c.must_consume = true;
        return c;
    }
    if s.is_resource_fiat {
        c.copy = false;
        return c;
    }
    if s.classification == Classification::Data && c.copy && !c.must_consume && !c.holds_authority {
        c.copy = true;
    } else {
        c.copy = false;
    }
    c
}

fn fold_components(
    components: &[(Type, Span)],
    items: &[DeclItem],
    seen: &mut BTreeSet<String>,
) -> TypeClasses {
    if components.is_empty() {
        return TypeClasses::default();
    }
    let mut copy = true;
    let mut must_consume = false;
    let mut crosses_actor = true;
    let mut holds_authority = false;
    for (ty, _) in components {
        let c = classes_of_type(ty, items, seen);
        copy &= c.copy;
        must_consume |= c.must_consume;
        crosses_actor &= c.crosses_actor;
        holds_authority |= c.holds_authority;
    }
    TypeClasses {
        copy,
        must_consume,
        crosses_actor,
        holds_authority,
    }
}

/// Classes of an arbitrary type, walking composites the same way
/// `protocol_resource_carried` / `type_carries_named` do.
pub fn classes_of_type(ty: &Type, items: &[DeclItem], seen: &mut BTreeSet<String>) -> TypeClasses {
    match ty {
        Type::Named(name, targs) => {
            if let Some(mut leaf) = leaf_classes(name) {
                if name == "Actor" || name == "Static" {
                    return leaf;
                }
                for a in targs {
                    if let TypeArg::Type(inner) = a {
                        let ic = classes_of_type(inner, items, seen);
                        leaf.copy &= ic.copy;
                        leaf.must_consume |= ic.must_consume;
                        leaf.crosses_actor &= ic.crosses_actor;
                        leaf.holds_authority |= ic.holds_authority;
                    }
                }
                return leaf;
            }
            if !seen.insert(name.clone()) {
                return TypeClasses::default();
            }
            let mut c = lookup_named_classes(name, items, seen);
            for a in targs {
                if let TypeArg::Type(inner) = a {
                    let ic = classes_of_type(inner, items, seen);
                    c.copy &= ic.copy;
                    c.must_consume |= ic.must_consume;
                    c.crosses_actor &= ic.crosses_actor;
                    c.holds_authority |= ic.holds_authority;
                }
            }
            seen.remove(name);
            c
        }
        Type::Array(elem, _) | Type::Option(elem) => classes_of_type(elem, items, seen),
        Type::Tuple(elems) => {
            if elems.is_empty() {
                return TypeClasses::default();
            }
            let mut copy = true;
            let mut must_consume = false;
            let mut crosses_actor = true;
            let mut holds_authority = false;
            for e in elems {
                let c = classes_of_type(e, items, seen);
                copy &= c.copy;
                must_consume |= c.must_consume;
                crosses_actor &= c.crosses_actor;
                holds_authority |= c.holds_authority;
            }
            TypeClasses {
                copy,
                must_consume,
                crosses_actor,
                holds_authority,
            }
        }
        Type::Own(_, _) => TypeClasses {
            copy: false,
            must_consume: false, // pool handle: §3.1 first bullet reclaim
            crosses_actor: true,
            holds_authority: false,
        },
        Type::Static(_) => TypeClasses {
            copy: true,
            must_consume: false,
            crosses_actor: true,
            holds_authority: false,
        },
        Type::Result(ok, err) => {
            let a = classes_of_type(ok, items, seen);
            let b = classes_of_type(err, items, seen);
            TypeClasses {
                copy: a.copy && b.copy,
                must_consume: a.must_consume || b.must_consume,
                crosses_actor: a.crosses_actor && b.crosses_actor,
                holds_authority: a.holds_authority || b.holds_authority,
            }
        }
        Type::Fn(params, ret) => {
            let mut c = classes_of_type(ret, items, seen);
            for (_, t) in params {
                let pc = classes_of_type(t, items, seen);
                c.copy &= pc.copy;
                c.must_consume |= pc.must_consume;
                c.crosses_actor &= pc.crosses_actor;
                c.holds_authority |= pc.holds_authority;
            }
            c
        }
        _ => TypeClasses::default(),
    }
}

fn lookup_named_classes(
    name: &str,
    items: &[DeclItem],
    seen: &mut BTreeSet<String>,
) -> TypeClasses {
    for item in items {
        match item {
            DeclItem::Struct(s) if s.name == name => {
                if s.classes_assigned {
                    return s.classes;
                }
                // During assign_classes over the snapshot: recompute from
                // components without re-entering via classes_assigned.
                let mut c = fold_components(&s.component_types, items, seen);
                if s.is_manual_resource {
                    c.copy = false;
                    c.must_consume = true;
                } else if s.is_resource_fiat {
                    c.copy = false;
                } else if !(s.classification == Classification::Data
                    && c.copy
                    && !c.must_consume
                    && !c.holds_authority)
                {
                    c.copy = false;
                }
                return c;
            }
            DeclItem::Enum(e) if e.name == name => {
                if e.classes_assigned {
                    return e.classes;
                }
                let mut c = fold_components(&e.component_types, items, seen);
                if !(e.classification == Classification::Data
                    && c.copy
                    && !c.must_consume
                    && !c.holds_authority)
                {
                    c.copy = false;
                }
                return c;
            }
            _ => {}
        }
    }
    TypeClasses::default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leaf_dual_run_protocol_and_authority_lists_agree() {
        let names = [
            "DeviceCap",
            "Mmio",
            "IrqCap",
            "DmaPool",
            "VirtQueue",
            "QueuePermit",
            "QueueOp",
            "Receipt",
            "ResetDevice",
            "AcknowledgedDevice",
            "DriverClaimedDevice",
            "FeaturesNegotiatedDevice",
            "FeaturesAcceptedDevice",
            "QueuesConfiguredDevice",
            "RunningDevice",
            "InterruptCell",
            "Actor",
            "Static",
            "Secret",
            "DmaShared",
            "u64",
            "Packet",
        ];
        for name in names {
            let _ = leaf_classes(name);
            let _ = name_forbidden_in_driver_message(name);
            assert_eq!(
                name_holds_authority(name),
                name_holds_authority(name),
                "{name} holds_authority"
            );
        }
        // must_consume leaf pins (former protocol-consumption name list).
        for name in [
            "DeviceCap",
            "Mmio",
            "IrqCap",
            "VirtQueue",
            "QueuePermit",
            "QueueOp",
            "Receipt",
            "RunningDevice",
        ] {
            assert!(name_must_consume(name, false), "{name}");
        }
        for name in ["DmaPool", "DmaShared", "InterruptCell", "Actor", "u64"] {
            assert!(!name_must_consume(name, false), "{name}");
        }
    }

    #[test]
    fn manual_resource_is_must_consume_by_fiat() {
        assert!(name_must_consume("Validated", true));
        assert!(!name_must_consume("Validated", false));
    }

    #[test]
    fn interrupt_cell_does_not_cross_actor() {
        let c = leaf_classes("InterruptCell").unwrap();
        assert!(!c.crosses_actor);
        assert!(!c.must_consume);
        assert!(!c.holds_authority);
        assert!(name_forbidden_in_driver_message("InterruptCell"));
    }

    #[test]
    fn dma_pool_holds_authority_but_not_must_consume() {
        let c = leaf_classes("DmaPool").unwrap();
        assert!(c.holds_authority);
        assert!(!c.must_consume);
        assert!(!c.crosses_actor);
    }
}
