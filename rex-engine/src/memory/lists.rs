//! Free-standing helpers for Rex list representations.

use std::sync::Arc;

use rex_ast::Symbol;

use crate::EngineError;

use super::heap::{Cell, Heap, InternalPtr, RootScope, RootedPtr};

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum ListElement {
    InternalPtr(InternalPtr),
    U8(u8),
}
#[derive(Clone, Copy)]
pub(super) enum ListRootElement {
    RootedPtr(RootedPtr),
    U8(u8),
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum ListItems<P> {
    Slice {
        elements: P,
        start: usize,
        end: usize,
    },
    BinarySlice {
        elements: P,
        start: usize,
        end: usize,
        bytes: Arc<[u8]>,
    },
    Pointers(Vec<P>),
}

impl<P> ListItems<P> {
    pub(crate) fn map_values<Q, E>(
        self,
        map: &mut impl FnMut(P) -> Result<Q, E>,
    ) -> Result<ListItems<Q>, E> {
        match self {
            Self::Slice {
                elements,
                start,
                end,
            } => Ok(ListItems::Slice {
                elements: map(elements)?,
                start,
                end,
            }),
            Self::BinarySlice {
                elements,
                start,
                end,
                bytes,
            } => Ok(ListItems::BinarySlice {
                elements: map(elements)?,
                start,
                end,
                bytes,
            }),
            Self::Pointers(values) => Ok(ListItems::Pointers(
                values.into_iter().map(map).collect::<Result<Vec<_>, _>>()?,
            )),
        }
    }

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
}

impl ListItems<InternalPtr> {
    pub(crate) fn into_rooted(self, scope: &mut RootScope<'_>) -> ListItems<RootedPtr> {
        match self {
            Self::Slice {
                elements,
                start,
                end,
            } => ListItems::Slice {
                elements: scope.root(elements),
                start,
                end,
            },
            Self::BinarySlice {
                elements,
                start,
                end,
                bytes,
            } => ListItems::BinarySlice {
                elements: scope.root(elements),
                start,
                end,
                bytes,
            },
            Self::Pointers(pointers) => {
                ListItems::Pointers(pointers.into_iter().map(|ptr| scope.root(ptr)).collect())
            }
        }
    }
}

impl ListItems<RootedPtr> {
    pub(crate) fn get(
        &self,
        scope: &mut RootScope<'_>,
        index: usize,
    ) -> Result<RootedPtr, EngineError> {
        match self {
            Self::Slice {
                elements,
                start,
                end,
            } => {
                let backing_index = start.checked_add(index).ok_or_else(|| {
                    EngineError::Internal("list slice backing index overflow".into())
                })?;
                if backing_index >= *end {
                    return Err(EngineError::Internal(
                        "list item index out of bounds".into(),
                    ));
                }
                let values = scope.get_cell_from_rooted_ptr(*elements)?.cell_as_data()?;
                values
                    .get(backing_index)
                    .copied()
                    .map(|value| scope.root(value))
                    .ok_or_else(|| {
                        EngineError::Internal("list slice backing index out of bounds".into())
                    })
            }
            Self::BinarySlice {
                start, end, bytes, ..
            } => {
                if index >= end - start {
                    return Err(EngineError::Internal(
                        "binary list slice index out of bounds".into(),
                    ));
                }
                let value = bytes.get(index).copied().ok_or_else(|| {
                    EngineError::Internal("binary list slice index out of bounds".into())
                })?;
                scope.alloc_root_u8(value)
            }
            Self::Pointers(values) => values
                .get(index)
                .copied()
                .ok_or_else(|| EngineError::Internal("list item index out of bounds".into())),
        }
    }
}

pub(super) enum ListItemsSeed {
    Ready(ListItems<InternalPtr>),
    Elements(Vec<ListElement>),
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
    heap: &Heap,
    elements: &InternalPtr,
    start: usize,
    end: usize,
    out: &mut Vec<ListElement>,
) -> Result<(), EngineError> {
    match heap.get_cell_from_pointer(elements)? {
        Cell::Data(values) => {
            validate_list_slice_bounds(values.len(), start, end)?;
            out.extend(
                values[start..end]
                    .iter()
                    .copied()
                    .map(ListElement::InternalPtr),
            );
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
    scope: &mut RootScope<'_>,
    elements: RootedPtr,
    start: usize,
    end: usize,
) -> Result<Option<ListRootElement>, EngineError> {
    if start >= end {
        return Ok(None);
    }
    match scope.get_cell_from_rooted_ptr(elements)? {
        Cell::Data(values) => {
            validate_list_slice_bounds(values.len(), start, end)?;
            Ok(values
                .get(start)
                .copied()
                .map(|x| ListRootElement::RootedPtr(scope.root(x))))
        }
        Cell::BinaryData(values) => {
            validate_list_slice_bounds(values.len(), start, end)?;
            Ok(values.get(start).copied().map(ListRootElement::U8))
        }
        cell => Err(EngineError::NativeType {
            expected: "list slice backing data".into(),
            got: cell.cell_type_name().into(),
        }),
    }
}

fn list_elements_from_cell(heap: &Heap, cell: &Cell) -> Result<Vec<ListElement>, EngineError> {
    let mut out = Vec::new();
    let mut cursor = cell;
    loop {
        match cursor {
            Cell::Empty => return Ok(out),
            Cell::Cons(head, tail) => {
                out.push(ListElement::InternalPtr(*head));
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
    heap: &Heap,
    pointer: InternalPtr,
) -> Result<Vec<ListElement>, EngineError> {
    let cell = heap.get_cell_from_pointer(&pointer)?;
    list_elements_from_cell(heap, cell)
}

pub(super) fn list_len_from_pointer(
    heap: &Heap,
    pointer: InternalPtr,
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
    scope: &mut RootScope<'_>,
    elements: Vec<ListElement>,
) -> Result<Vec<RootedPtr>, EngineError> {
    let mut pointer_roots = elements
        .iter()
        .filter_map(|element| match element {
            ListElement::InternalPtr(pointer) => Some(scope.root(*pointer)),
            ListElement::U8(_) => None,
        })
        .collect::<Vec<_>>()
        .into_iter();

    let mut result = Vec::with_capacity(elements.len());
    for element in elements {
        match element {
            ListElement::InternalPtr(_) => {
                let rooted = pointer_roots.next().ok_or_else(|| {
                    EngineError::Internal("missing pre-rooted list pointer".into())
                })?;
                result.push(rooted);
            }
            ListElement::U8(value) => {
                result.push(scope.alloc_root_u8(value)?);
            }
        }
    }

    Ok(result)
}

pub(super) fn list_elements_to_rooted_ptr_vec(
    scope: &mut RootScope<'_>,
    elements: Vec<ListElement>,
) -> Result<Vec<RootedPtr>, EngineError> {
    elements
        .into_iter()
        .map(|element| match element {
            ListElement::InternalPtr(pointer) => Ok(scope.root(pointer)),
            ListElement::U8(_) => Err(EngineError::NativeType {
                expected: "pointer-backed list".into(),
                got: "binary-backed list".into(),
            }),
        })
        .collect()
}

pub(super) fn collect_list_u8(heap: &Heap, pointer: &InternalPtr) -> Result<Vec<u8>, EngineError> {
    let mut out = Vec::with_capacity(list_len_from_pointer(heap, *pointer)?);
    let mut cursor = heap.get_cell_from_pointer(pointer)?;
    loop {
        match cursor {
            Cell::Empty => return Ok(out),
            Cell::Cons(head, tail) => {
                out.push(heap.get_cell_from_pointer(head)?.cell_as_u8()?);
                cursor = heap.get_cell_from_pointer(tail)?;
            }
            Cell::ListSlice {
                start,
                end,
                elements,
            } => match heap.get_cell_from_pointer(elements)? {
                Cell::Data(values) => {
                    validate_list_slice_bounds(values.len(), *start, *end)?;
                    for value in &values[*start..*end] {
                        out.push(heap.get_cell_from_pointer(value)?.cell_as_u8()?);
                    }
                    return Ok(out);
                }
                Cell::BinaryData(values) => {
                    validate_list_slice_bounds(values.len(), *start, *end)?;
                    out.extend_from_slice(&values[*start..*end]);
                    return Ok(out);
                }
                cell => {
                    return Err(EngineError::NativeType {
                        expected: "list slice backing data".into(),
                        got: cell.cell_type_name().into(),
                    });
                }
            },
            cell => {
                return Err(EngineError::NativeType {
                    expected: "list".into(),
                    got: cell.cell_type_name().into(),
                });
            }
        }
    }
}

pub(super) fn list_items_from_pointer(
    heap: &Heap,
    pointer: InternalPtr,
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
