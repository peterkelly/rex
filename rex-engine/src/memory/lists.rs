//! Free-standing helpers for Rex list representations.

use std::{collections::HashSet, sync::Arc};

use rex_ast::Symbol;

use crate::EngineError;

use super::{
    heap::{
        Cell, Heap, HeapState, Pointer, PointerKey, PointerPairKey, ValueDisplayOptions,
        pointer_debug_inner, pointer_display_inner, pointer_eq_inner,
    },
    traits::Collection,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum ListElement {
    Pointer(Pointer),
    U8(u8),
}

enum MaterializedListElement {
    RootedPointer(usize),
    RootedByte(usize),
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum ListItems {
    Slice {
        elements: Pointer,
        start: usize,
        end: usize,
    },
    BinarySlice {
        elements: Pointer,
        start: usize,
        end: usize,
        bytes: Arc<[u8]>,
    },
    Pointers(Vec<Pointer>),
}

impl ListItems {
    pub(crate) fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub(crate) fn len(&self) -> usize {
        match self {
            Self::Slice { start, end, .. } => end - start,
            Self::BinarySlice { start, end, .. } => end - start,
            Self::Pointers(values) => values.len(),
        }
    }

    pub(crate) fn get(&self, heap: &mut HeapState, index: usize) -> Result<Pointer, EngineError> {
        match self {
            Self::Slice {
                elements,
                start,
                end,
            } => {
                let len = end - start;
                if index >= len {
                    return Err(EngineError::Internal(
                        "list item index out of bounds".into(),
                    ));
                }
                let backing_index = start.checked_add(index).ok_or_else(|| {
                    EngineError::Internal("list slice backing index overflow".into())
                })?;
                if backing_index >= *end {
                    return Err(EngineError::Internal(
                        "list slice backing index out of bounds".into(),
                    ));
                }
                let values = heap.get_cell_from_pointer(elements)?.cell_as_data()?;
                values.get(backing_index).copied().ok_or_else(|| {
                    EngineError::Internal("list slice backing index out of bounds".into())
                })
            }
            Self::BinarySlice {
                start, end, bytes, ..
            } => {
                let len = end - start;
                if index >= len {
                    return Err(EngineError::Internal(
                        "list item index out of bounds".into(),
                    ));
                }
                let value = bytes.get(index).copied().ok_or_else(|| {
                    EngineError::Internal("binary list slice index out of bounds".into())
                })?;
                Ok(heap.alloc_ptr_u8(value)?.into_pointer())
            }
            Self::Pointers(values) => values
                .get(index)
                .copied()
                .ok_or_else(|| EngineError::Internal("list item index out of bounds".into())),
        }
    }
}

pub(super) enum ListItemsSeed {
    Ready(ListItems),
    Elements(Vec<ListElement>),
}

impl Collection for ListItems {
    fn map_pointers<E>(
        &mut self,
        map: &mut impl FnMut(Pointer) -> Result<Pointer, E>,
    ) -> Result<(), E> {
        match self {
            Self::Slice { elements, .. } | Self::BinarySlice { elements, .. } => {
                *elements = map(*elements)?;
                Ok(())
            }
            Self::Pointers(values) => {
                for pointer in values {
                    *pointer = map(*pointer)?;
                }
                Ok(())
            }
        }
    }
}

fn usize_to_i32_saturating(index: usize) -> i32 {
    i32::try_from(index).unwrap_or(i32::MAX)
}

pub(super) fn validate_list_slice_bounds(
    data_len: usize,
    start: usize,
    end: usize,
) -> Result<(), EngineError> {
    if start > end {
        return Err(EngineError::Custom(format!(
            "invalid list slice range: start {start} is greater than end {end}"
        )));
    }
    if end > data_len {
        return Err(EngineError::IndexOutOfBounds {
            name: Symbol::intern("ListSlice"),
            index: usize_to_i32_saturating(end),
            len: data_len,
        });
    }
    Ok(())
}

pub(super) fn list_slice_backing_len(cell: &Cell) -> Result<usize, EngineError> {
    match cell {
        Cell::Data(values) => Ok(values.len()),
        Cell::BinaryData(values) => Ok(values.len()),
        _ => Err(EngineError::NativeType {
            expected: "list slice backing data".into(),
            got: cell.cell_type_name().into(),
        }),
    }
}

fn append_list_slice_elements(
    heap: &HeapState,
    elements: &Pointer,
    start: usize,
    end: usize,
    out: &mut Vec<ListElement>,
) -> Result<(), EngineError> {
    match heap.get_cell_from_pointer(elements)? {
        Cell::Data(values) => {
            validate_list_slice_bounds(values.len(), start, end)?;
            out.extend(values[start..end].iter().copied().map(ListElement::Pointer));
            Ok(())
        }
        Cell::BinaryData(values) => {
            validate_list_slice_bounds(values.len(), start, end)?;
            out.extend(values[start..end].iter().copied().map(ListElement::U8));
            Ok(())
        }
        cell => Err(EngineError::NativeType {
            expected: "list slice backing data".into(),
            got: cell.cell_type_name().into(),
        }),
    }
}

pub(super) fn list_slice_head_element(
    heap: &HeapState,
    elements: &Pointer,
    start: usize,
    end: usize,
) -> Result<Option<ListElement>, EngineError> {
    if start >= end {
        return Ok(None);
    }
    match heap.get_cell_from_pointer(elements)? {
        Cell::Data(values) => {
            validate_list_slice_bounds(values.len(), start, end)?;
            Ok(values.get(start).copied().map(ListElement::Pointer))
        }
        Cell::BinaryData(values) => {
            validate_list_slice_bounds(values.len(), start, end)?;
            Ok(values.get(start).copied().map(ListElement::U8))
        }
        cell => Err(EngineError::NativeType {
            expected: "list slice backing data".into(),
            got: cell.cell_type_name().into(),
        }),
    }
}

fn list_elements_from_cell(heap: &HeapState, cell: &Cell) -> Result<Vec<ListElement>, EngineError> {
    let mut out = Vec::new();
    let mut cursor = cell;
    loop {
        match cursor {
            Cell::Empty => return Ok(out),
            Cell::Cons(head, tail) => {
                out.push(ListElement::Pointer(*head));
                cursor = heap.get_cell_from_pointer(tail)?;
            }
            Cell::ListSlice {
                start,
                end,
                elements,
            } => {
                append_list_slice_elements(heap, elements, *start, *end, &mut out)?;
                return Ok(out);
            }
            _ => {
                return Err(EngineError::NativeType {
                    expected: "list".into(),
                    got: cursor.cell_type_name().into(),
                });
            }
        }
    }
}

pub(super) fn list_elements_from_pointer(
    heap: &HeapState,
    pointer: Pointer,
) -> Result<Vec<ListElement>, EngineError> {
    let cell = heap.get_cell_from_pointer(&pointer)?;
    list_elements_from_cell(heap, cell)
}

pub(super) fn list_len_from_pointer(
    heap: &HeapState,
    pointer: Pointer,
) -> Result<usize, EngineError> {
    let mut len = 0usize;
    let mut cursor = heap.get_cell_from_pointer(&pointer)?;
    loop {
        match cursor {
            Cell::Empty => return Ok(len),
            Cell::Cons(_, tail) => {
                len = len
                    .checked_add(1)
                    .ok_or_else(|| EngineError::Internal("list length overflow".into()))?;
                cursor = heap.get_cell_from_pointer(tail)?;
            }
            Cell::ListSlice {
                start,
                end,
                elements,
            } => {
                let backing_len = list_slice_backing_len(heap.get_cell_from_pointer(elements)?)?;
                validate_list_slice_bounds(backing_len, *start, *end)?;
                return len
                    .checked_add(end - start)
                    .ok_or_else(|| EngineError::Internal("list length overflow".into()));
            }
            cell => {
                return Err(EngineError::NativeType {
                    expected: "list".into(),
                    got: cell.cell_type_name().into(),
                });
            }
        }
    }
}

pub(super) fn materialize_list_elements(
    heap: &Heap,
    elements: Vec<ListElement>,
) -> Result<Vec<Pointer>, EngineError> {
    let pointer_values = elements
        .iter()
        .filter_map(|element| match element {
            ListElement::Pointer(pointer) => Some(*pointer),
            ListElement::U8(_) => None,
        })
        .collect::<Vec<_>>();
    let pointer_roots = heap.temp_roots(pointer_values)?;
    let mut next_pointer_root = 0;
    let mut byte_roots = Vec::new();
    let mut materialized = Vec::with_capacity(elements.len());

    for element in elements {
        match element {
            ListElement::Pointer(_) => {
                materialized.push(MaterializedListElement::RootedPointer(next_pointer_root));
                next_pointer_root += 1;
            }
            ListElement::U8(value) => {
                let pointer =
                    heap.with_locked(|heap| Ok(heap.alloc_ptr_u8(value)?.into_pointer()))?;
                byte_roots.push(heap.temp_roots(vec![pointer])?);
                materialized.push(MaterializedListElement::RootedByte(byte_roots.len() - 1));
            }
        }
    }

    materialized
        .into_iter()
        .map(|element| match element {
            MaterializedListElement::RootedPointer(index) => pointer_roots.get(index),
            MaterializedListElement::RootedByte(index) => byte_roots[index].get(0),
        })
        .collect()
}

pub(super) fn list_elements_to_pointer_vec(
    elements: Vec<ListElement>,
) -> Result<Vec<Pointer>, EngineError> {
    elements
        .into_iter()
        .map(|element| match element {
            ListElement::Pointer(pointer) => Ok(pointer),
            ListElement::U8(_) => Err(EngineError::NativeType {
                expected: "pointer-backed list".into(),
                got: "binary-backed list".into(),
            }),
        })
        .collect()
}

pub(crate) fn list_to_vec(heap: &HeapState, cell: &Cell) -> Result<Vec<Pointer>, EngineError> {
    list_elements_to_pointer_vec(list_elements_from_cell(heap, cell)?)
}

pub(super) fn collect_list_u8(heap: &HeapState, pointer: &Pointer) -> Result<Vec<u8>, EngineError> {
    let elements = list_elements_from_pointer(heap, *pointer)?;
    let mut out = Vec::with_capacity(elements.len());
    for element in elements {
        match element {
            ListElement::Pointer(pointer) => out.push(heap.pointer_as_u8(&pointer)?),
            ListElement::U8(value) => out.push(value),
        }
    }
    Ok(out)
}

pub(super) fn format_list_debug(
    heap: &HeapState,
    cell: &Cell,
    active: &mut HashSet<PointerKey>,
) -> Result<String, EngineError> {
    let items = list_elements_from_cell(heap, cell)?
        .into_iter()
        .map(|element| match element {
            ListElement::Pointer(pointer) => pointer_debug_inner(heap, &pointer, active),
            ListElement::U8(value) => Ok(format!("{value}u8")),
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(format!("[{}]", items.join(", ")))
}

pub(super) fn format_list_display(
    heap: &HeapState,
    cell: &Cell,
    active: &mut HashSet<PointerKey>,
    opts: ValueDisplayOptions,
) -> Result<String, EngineError> {
    let items = list_elements_from_cell(heap, cell)?
        .into_iter()
        .map(|element| match element {
            ListElement::Pointer(pointer) => pointer_display_inner(heap, &pointer, active, opts),
            ListElement::U8(value) => {
                if opts.include_numeric_suffixes {
                    Ok(format!("{value}u8"))
                } else {
                    Ok(value.to_string())
                }
            }
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(format!("[{}]", items.join(", ")))
}

fn list_element_eq_inner(
    heap: &HeapState,
    lhs: ListElement,
    rhs: ListElement,
    seen: &mut HashSet<PointerPairKey>,
) -> Result<bool, EngineError> {
    match (lhs, rhs) {
        (ListElement::Pointer(lhs), ListElement::Pointer(rhs)) => {
            pointer_eq_inner(heap, &lhs, &rhs, seen)
        }
        (ListElement::U8(lhs), ListElement::U8(rhs)) => Ok(lhs == rhs),
        (ListElement::U8(lhs), ListElement::Pointer(rhs))
        | (ListElement::Pointer(rhs), ListElement::U8(lhs)) => {
            match heap.get_cell_from_pointer(&rhs)? {
                Cell::U8(rhs) => Ok(lhs == *rhs),
                _ => Ok(false),
            }
        }
    }
}

pub(super) fn list_cells_eq_inner(
    heap: &HeapState,
    lhs: &Cell,
    rhs: &Cell,
    seen: &mut HashSet<PointerPairKey>,
) -> Result<bool, EngineError> {
    let lhs = list_elements_from_cell(heap, lhs)?;
    let rhs = list_elements_from_cell(heap, rhs)?;
    if lhs.len() != rhs.len() {
        return Ok(false);
    }
    for (lhs, rhs) in lhs.into_iter().zip(rhs) {
        if !list_element_eq_inner(heap, lhs, rhs, seen)? {
            return Ok(false);
        }
    }
    Ok(true)
}

pub(super) fn list_items_from_pointer(
    heap: &HeapState,
    pointer: Pointer,
) -> Result<ListItemsSeed, EngineError> {
    let cell = heap.get_cell_from_pointer(&pointer)?;
    match cell {
        Cell::Empty => Ok(ListItemsSeed::Ready(ListItems::Pointers(Vec::new()))),
        Cell::ListSlice {
            start,
            end,
            elements,
        } => match heap.get_cell_from_pointer(elements)? {
            Cell::Data(values) => {
                validate_list_slice_bounds(values.len(), *start, *end)?;
                Ok(ListItemsSeed::Ready(ListItems::Slice {
                    elements: *elements,
                    start: *start,
                    end: *end,
                }))
            }
            Cell::BinaryData(values) => {
                validate_list_slice_bounds(values.len(), *start, *end)?;
                Ok(ListItemsSeed::Ready(ListItems::BinarySlice {
                    elements: *elements,
                    start: *start,
                    end: *end,
                    bytes: Arc::from(&values[*start..*end]),
                }))
            }
            cell => Err(EngineError::NativeType {
                expected: "list slice backing data".into(),
                got: cell.cell_type_name().into(),
            }),
        },
        Cell::Cons(..) => Ok(ListItemsSeed::Elements(list_elements_from_cell(
            heap, cell,
        )?)),
        _ => Err(EngineError::NativeType {
            expected: "list".into(),
            got: cell.cell_type_name().into(),
        }),
    }
}
