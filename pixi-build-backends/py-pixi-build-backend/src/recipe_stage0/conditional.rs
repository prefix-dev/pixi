use pixi_build_backend::package_dependency::PackageDependency;
use pyo3::exceptions::PyTypeError;
use pyo3::types::{PyAnyMethods, PyList, PyListMethods};
use pyo3::{Bound, FromPyObject, Py, PyAny, PyErr, PyResult, Python, intern, pyclass, pymethods};
use rattler_build_recipe::stage0::{
    Conditional, Item, JinjaExpression, JinjaTemplate, ListOrItem, NestedItemList, Source, Value,
};
use serde::{Deserialize, Serialize};
use std::fmt::Display;
use std::ops::Deref;
use std::str::FromStr;

use crate::create_py_wrap;
use crate::recipe_stage0::recipe::PySource;
use crate::recipe_stage0::requirements::PyPackageDependency;

/// Helper to parse a string into an `Item<T>`.
///
/// If the string contains `${{`, it is treated as a Jinja template.
/// Otherwise, it is parsed as a concrete value of type `T` via `FromStr`.
fn parse_item_from_str<T: FromStr>(s: &str) -> Result<Item<T>, String>
where
    T::Err: Display,
{
    if s.contains("${{") {
        let template = JinjaTemplate::new(s.to_string())
            .map_err(|e| format!("invalid jinja template: {e}"))?;
        Ok(Item::Value(Value::new_template(template, None)))
    } else {
        let value = T::from_str(s).map_err(|e| format!("failed to parse value: {e}"))?;
        Ok(Item::Value(Value::new_concrete(value, None)))
    }
}

/// Internal macro that generates the common PyItem body shared by all
/// variants.  Not meant to be invoked directly -- use `create_py_item!`
/// or `create_py_item_no_parse!` instead.
macro_rules! _create_py_item_common {
    ($name: ident, $type: ident, $py_type: ident) => {
        paste::paste! {
            #[pyclass]
            #[derive(Clone, Serialize, Deserialize)]
            pub struct $name {
                pub(crate) inner: Item<$type>,
            }

            impl From<Item<$type>> for $name {
                fn from(item: Item<$type>) -> Self {
                    $name { inner: item }
                }
            }

            impl Deref for $name {
                type Target = Item<$type>;
                fn deref(&self) -> &Self::Target {
                    &self.inner
                }
            }
        }
    };
}

/// Creates a PyItem class for types that implement `FromStr + Display`.
macro_rules! create_py_item {
    ($name: ident, $type: ident, $py_type: ident) => {
        _create_py_item_common!($name, $type, $py_type);

        paste::paste! {
            #[pymethods]
            impl $name {
                #[new]
                pub fn new(value: String) -> PyResult<Self> {
                    let item = parse_item_from_str::<$type>(&value)
                        .map_err(|e| PyTypeError::new_err(format!("failed to parse {value}: {e}")))?;

                    Ok($name { inner: item })
                }

                #[staticmethod]
                pub fn new_from_conditional(
                    py: Python,
                    conditional: [<PyConditional $type>]
                ) -> Self {
                    let conditional = [<PyConditional $type>]::into_conditional(py, conditional);
                    let item = Item::Conditional(conditional);
                    $name { inner: item }
                }

                pub fn is_value(&self) -> bool {
                    matches!(self.inner, Item::Value(_))
                }

                pub fn is_template(&self) -> bool {
                    match &self.inner {
                        Item::Value(v) => v.is_template(),
                        _ => false,
                    }
                }

                pub fn is_conditional(&self) -> bool {
                    matches!(self.inner, Item::Conditional(_))
                }

                pub fn __str__(&self) -> String {
                    self.inner.to_string()
                }

                pub fn concrete(&self) -> Option<$py_type> {
                    if let Item::Value(val) = &self.inner {
                        val.as_concrete().map(|c| c.clone().into())
                    } else {
                        None
                    }
                }

                pub fn template(&self) -> Option<String> {
                    if let Item::Value(val) = &self.inner {
                        val.as_template().map(|t| t.to_string())
                    } else {
                        None
                    }
                }

                pub fn conditional(&self, py: Python) -> Option<[<PyConditional $type>]> {
                    if let Item::Conditional(cond) = &self.inner {
                        Some([<PyConditional $type>]::from_conditional(py, cond.clone()))
                    } else {
                        None
                    }
                }
            }

            impl Display for $name {
                fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                    write!(f, "{}", self.inner)
                }
            }
        }
    };
}

/// Creates a PyItem class for types that do NOT implement `FromStr` or `Display`
/// (e.g. `Source`).  The `#[new]` constructor is omitted and `__str__` uses
/// a simple debug-style representation.
macro_rules! create_py_item_no_parse {
    ($name: ident, $type: ident, $py_type: ident) => {
        _create_py_item_common!($name, $type, $py_type);

        paste::paste! {
            #[pymethods]
            impl $name {
                #[staticmethod]
                pub fn new_from_conditional(
                    py: Python,
                    conditional: [<PyConditional $type>]
                ) -> Self {
                    let conditional = [<PyConditional $type>]::into_conditional(py, conditional);
                    let item = Item::Conditional(conditional);
                    $name { inner: item }
                }

                #[staticmethod]
                pub fn new_from_value(value: $py_type) -> Self {
                    $name {
                        inner: Item::Value(Value::new_concrete(value.inner, None)),
                    }
                }

                pub fn is_value(&self) -> bool {
                    matches!(self.inner, Item::Value(_))
                }

                pub fn is_template(&self) -> bool {
                    match &self.inner {
                        Item::Value(v) => v.is_template(),
                        _ => false,
                    }
                }

                pub fn is_conditional(&self) -> bool {
                    matches!(self.inner, Item::Conditional(_))
                }

                pub fn __str__(&self) -> String {
                    match &self.inner {
                        Item::Value(v) => {
                            if let Some(t) = v.as_template() {
                                t.to_string()
                            } else {
                                "<source>".to_string()
                            }
                        }
                        Item::Conditional(_) => "<conditional>".to_string(),
                    }
                }

                pub fn concrete(&self) -> Option<$py_type> {
                    if let Item::Value(val) = &self.inner {
                        val.as_concrete().map(|c| c.clone().into())
                    } else {
                        None
                    }
                }

                pub fn template(&self) -> Option<String> {
                    if let Item::Value(val) = &self.inner {
                        val.as_template().map(|t| t.to_string())
                    } else {
                        None
                    }
                }

                pub fn conditional(&self, py: Python) -> Option<[<PyConditional $type>]> {
                    if let Item::Conditional(cond) = &self.inner {
                        Some([<PyConditional $type>]::from_conditional(py, cond.clone()))
                    } else {
                        None
                    }
                }
            }

            impl Display for $name {
                fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                    match &self.inner {
                        Item::Value(v) => {
                            if let Some(t) = v.as_template() {
                                write!(f, "{t}")
                            } else {
                                write!(f, "<source>")
                            }
                        }
                        Item::Conditional(_) => write!(f, "<conditional>"),
                    }
                }
            }
        }
    };
}

create_py_item!(
    PyItemPackageDependency,
    PackageDependency,
    PyPackageDependency
);
create_py_item!(PyItemString, String, String);
create_py_item_no_parse!(PyItemSource, Source, PySource);

create_py_wrap!(PyVecString, Vec<String>, |v: &Vec<String>,
                                           f: &mut std::fmt::Formatter<
    '_,
>| {
    write!(f, "[{}]", v.join(", "))
});

macro_rules! create_pylist_or_item {
    ($name: ident, $type: ident, $py_type: ident) => {
        #[pyclass(str, eq)]
        #[derive(Clone, PartialEq, Deserialize, Serialize)]
        pub struct $name {
            pub(crate) inner: ListOrItem<$type>,
        }

        #[pymethods]
        impl $name {
            #[new]
            pub fn new(item: Vec<String>) -> Self {
                let item: Vec<$type> = item
                    .into_iter()
                    .map(|s| s.parse().expect("Failed to parse item"))
                    .collect();

                $name {
                    inner: ListOrItem::new(item),
                }
            }

            pub fn is_single(&self) -> bool {
                self.inner.len() == 1
            }

            pub fn is_list(&self) -> bool {
                self.inner.len() > 1
            }

            pub fn __getitem__(&self, index: usize) -> PyResult<$py_type> {
                Ok(Into::<$py_type>::into(
                    self.inner.as_slice().get(index).cloned().unwrap(),
                ))
            }

            pub fn __len__(&self) -> usize {
                self.inner.len()
            }

            pub fn __contains__(&self, item: $py_type) -> bool {
                let inner_item: $type = item.into();
                self.inner.as_slice().contains(&inner_item)
            }

            pub fn append(&mut self, item: $py_type) {
                let mut items =
                    std::mem::replace(&mut self.inner, ListOrItem::new(Vec::new())).into_vec();
                items.push(item.into());
                self.inner = ListOrItem::new(items);
            }

            pub fn extend(&mut self, items: &Bound<'_, PyList>) {
                let new_items: Vec<$type> = items
                    .iter()
                    .map(|item| item.extract::<$py_type>().unwrap().into())
                    .collect();
                let mut existing =
                    std::mem::replace(&mut self.inner, ListOrItem::new(Vec::new())).into_vec();
                existing.extend(new_items);
                self.inner = ListOrItem::new(existing);
            }

            pub fn insert(&mut self, index: usize, item: $py_type) {
                let mut items =
                    std::mem::replace(&mut self.inner, ListOrItem::new(Vec::new())).into_vec();
                items.insert(index, item.into());
                self.inner = ListOrItem::new(items);
            }

            pub fn remove(&mut self, item: $py_type) -> PyResult<()> {
                let target_item: $type = item.clone().into();
                let items = self.inner.as_slice();
                if let Some(pos) = items.iter().position(|x| *x == target_item) {
                    let mut items =
                        std::mem::replace(&mut self.inner, ListOrItem::new(Vec::new())).into_vec();
                    items.remove(pos);
                    self.inner = ListOrItem::new(items);
                    Ok(())
                } else {
                    Err(pyo3::exceptions::PyValueError::new_err("item not found"))
                }
            }

            pub fn pop(&mut self, index: Option<isize>) -> PyResult<$py_type> {
                let len = self.inner.len();
                if len == 0 {
                    return Err(pyo3::exceptions::PyIndexError::new_err(
                        "pop from empty list",
                    ));
                }

                let idx = match index {
                    Some(i) => {
                        let idx = if i < 0 {
                            (len as isize + i) as usize
                        } else {
                            i as usize
                        };
                        if idx >= len {
                            return Err(pyo3::exceptions::PyIndexError::new_err(
                                "pop index out of range",
                            ));
                        }
                        idx
                    }
                    None => len - 1,
                };

                let mut items =
                    std::mem::replace(&mut self.inner, ListOrItem::new(Vec::new())).into_vec();
                let removed = items.remove(idx);
                self.inner = ListOrItem::new(items);
                Ok(Into::<$py_type>::into(removed))
            }

            pub fn index(
                &self,
                item: $py_type,
                start: Option<usize>,
                end: Option<usize>,
            ) -> PyResult<usize> {
                let slice = self.inner.as_slice();
                let start = start.unwrap_or(0);
                let end = end.unwrap_or(slice.len());

                if start >= slice.len() {
                    return Err(pyo3::exceptions::PyValueError::new_err("item not found"));
                }

                for (i, existing_item) in slice[start..end.min(slice.len())].iter().enumerate() {
                    let target_item: $type = item.clone().into();
                    if *existing_item == target_item {
                        return Ok(start + i);
                    }
                }

                Err(pyo3::exceptions::PyValueError::new_err("item not found"))
            }

            pub fn count(&self, item: $py_type) -> usize {
                let target_item: $type = item.clone().into();
                self.inner
                    .as_slice()
                    .iter()
                    .filter(|&x| *x == target_item)
                    .count()
            }

            pub fn clear(&mut self) {
                self.inner = ListOrItem::new(Vec::new());
            }

            pub fn reverse(&mut self) {
                let mut items =
                    std::mem::replace(&mut self.inner, ListOrItem::new(Vec::new())).into_vec();
                items.reverse();
                self.inner = ListOrItem::new(items);
            }

            pub fn copy(&self) -> Self {
                self.clone()
            }

            pub fn __setitem__(&mut self, index: usize, value: $py_type) -> PyResult<()> {
                let mut items =
                    std::mem::replace(&mut self.inner, ListOrItem::new(Vec::new())).into_vec();
                if index >= items.len() {
                    self.inner = ListOrItem::new(items);
                    return Err(pyo3::exceptions::PyIndexError::new_err(
                        "list index out of range",
                    ));
                }
                items[index] = value.into();
                self.inner = ListOrItem::new(items);
                Ok(())
            }

            pub fn __delitem__(&mut self, index: usize) -> PyResult<()> {
                let mut items =
                    std::mem::replace(&mut self.inner, ListOrItem::new(Vec::new())).into_vec();
                if index >= items.len() {
                    self.inner = ListOrItem::new(items);
                    return Err(pyo3::exceptions::PyIndexError::new_err(
                        "list index out of range",
                    ));
                }
                items.remove(index);
                self.inner = ListOrItem::new(items);
                Ok(())
            }

            pub fn __iter__(
                slf: pyo3::pycell::PyRef<'_, Self>,
                py: Python,
            ) -> pyo3::PyResult<Py<PyList>> {
                let py_list = PyList::empty(py);
                for item in slf.inner.iter() {
                    let py_item: $py_type = item.clone().into();
                    py_list.append(py_item)?;
                }
                Ok(py_list.into())
            }
        }

        impl From<ListOrItem<$type>> for $name {
            fn from(inner: ListOrItem<$type>) -> Self {
                $name { inner }
            }
        }

        impl Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "[")?;
                let mut first = true;
                for item in self.inner.iter() {
                    if !first {
                        write!(f, ", ")?;
                    }
                    write!(f, "{item}")?;
                    first = false;
                }
                write!(f, "]")
            }
        }
    };
}

create_pylist_or_item!(PyListOrItemString, String, String);
create_pylist_or_item!(
    PyListOrItemPackageDependency,
    PackageDependency,
    PyPackageDependency
);
// NOTE: PyListOrItemSource is intentionally NOT created via create_pylist_or_item!
// because `Source` does not implement `FromStr` or `Display`.
// It is defined manually below with a reduced API.
#[pyclass(str, eq)]
#[derive(Clone, PartialEq, Deserialize, Serialize)]
pub struct PyListOrItemSource {
    pub(crate) inner: ListOrItem<Source>,
}

#[pymethods]
impl PyListOrItemSource {
    #[new]
    pub fn new(items: Vec<PySource>) -> Self {
        let items: Vec<Source> = items.into_iter().map(|s| s.inner).collect();
        PyListOrItemSource {
            inner: ListOrItem::new(items),
        }
    }

    pub fn is_single(&self) -> bool {
        self.inner.len() == 1
    }

    pub fn is_list(&self) -> bool {
        self.inner.len() > 1
    }

    pub fn __getitem__(&self, index: usize) -> PyResult<PySource> {
        Ok(PySource {
            inner: self.inner.as_slice().get(index).cloned().unwrap(),
        })
    }

    pub fn __len__(&self) -> usize {
        self.inner.len()
    }

    pub fn append(&mut self, item: PySource) {
        let mut items = std::mem::replace(&mut self.inner, ListOrItem::new(Vec::new())).into_vec();
        items.push(item.inner);
        self.inner = ListOrItem::new(items);
    }

    pub fn __iter__(slf: pyo3::pycell::PyRef<'_, Self>, py: Python) -> pyo3::PyResult<Py<PyList>> {
        let py_list = PyList::empty(py);
        for item in slf.inner.iter() {
            let py_item = PySource {
                inner: item.clone(),
            };
            py_list.append(py_item)?;
        }
        Ok(py_list.into())
    }
}

impl From<ListOrItem<Source>> for PyListOrItemSource {
    fn from(inner: ListOrItem<Source>) -> Self {
        PyListOrItemSource { inner }
    }
}

impl Display for PyListOrItemSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{} source(s)]", self.inner.len())
    }
}

create_py_wrap!(PyWrapString, String);

/// Creates a PyNestedItemList class for wrapping `NestedItemList<T>`.
/// Used for conditional then/else branches, which contain `Vec<Item<T>>`.
macro_rules! create_py_nested_item_list {
    ($name: ident, $type: ident, $py_item_type: ident) => {
        paste::paste! {
            #[pyclass(str, eq)]
            #[derive(Clone, PartialEq, Deserialize, Serialize)]
            pub struct $name {
                pub(crate) inner: NestedItemList<$type>,
            }

            #[pymethods]
            impl $name {
                #[new]
                pub fn new(items: Vec<$py_item_type>) -> Self {
                    let items: Vec<Item<$type>> = items
                        .into_iter()
                        .map(|item| item.inner)
                        .collect();
                    $name {
                        inner: NestedItemList::new(items),
                    }
                }

                pub fn __getitem__(&self, index: usize) -> PyResult<$py_item_type> {
                    Ok(Into::<$py_item_type>::into(
                        self.inner.as_slice().get(index).cloned().unwrap(),
                    ))
                }

                pub fn __setitem__(&mut self, index: usize, value: $py_item_type) -> PyResult<()> {
                    let mut items: Vec<Item<$type>> = self.inner.iter().cloned().collect();
                    if index >= items.len() {
                        return Err(pyo3::exceptions::PyIndexError::new_err(
                            "list index out of range",
                        ));
                    }
                    items[index] = value.inner;
                    self.inner = NestedItemList::new(items);
                    Ok(())
                }

                pub fn __delitem__(&mut self, index: usize) -> PyResult<()> {
                    let mut items: Vec<Item<$type>> = self.inner.iter().cloned().collect();
                    if index >= items.len() {
                        return Err(pyo3::exceptions::PyIndexError::new_err(
                            "list index out of range",
                        ));
                    }
                    items.remove(index);
                    self.inner = NestedItemList::new(items);
                    Ok(())
                }

                pub fn __len__(&self) -> usize {
                    self.inner.len()
                }

                pub fn extend(&mut self, items: &Bound<'_, PyList>) {
                    let new_items: Vec<Item<$type>> = items
                        .iter()
                        .map(|item| {
                            let py_item = item.extract::<$py_item_type>().unwrap();
                            py_item.inner
                        })
                        .collect();
                    let mut existing: Vec<Item<$type>> = self.inner.iter().cloned().collect();
                    existing.extend(new_items);
                    self.inner = NestedItemList::new(existing);
                }

                pub fn __iter__(
                    slf: pyo3::pycell::PyRef<'_, Self>,
                    py: Python,
                ) -> pyo3::PyResult<Py<PyList>> {
                    let py_list = PyList::empty(py);
                    for item in slf.inner.iter() {
                        let py_item: $py_item_type = item.clone().into();
                        py_list.append(py_item)?;
                    }
                    Ok(py_list.into())
                }
            }

            impl From<NestedItemList<$type>> for $name {
                fn from(inner: NestedItemList<$type>) -> Self {
                    $name { inner }
                }
            }

            impl Display for $name {
                fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                    write!(f, "[")?;
                    let mut first = true;
                    for item in self.inner.iter() {
                        if !first {
                            write!(f, ", ")?;
                        }
                        write!(f, "{item}")?;
                        first = false;
                    }
                    write!(f, "]")
                }
            }
        }
    };
}

create_py_nested_item_list!(PyNestedItemListString, String, PyItemString);
create_py_nested_item_list!(
    PyNestedItemListPackageDependency,
    PackageDependency,
    PyItemPackageDependency
);
// NOTE: PyNestedItemListSource is intentionally NOT created via
// create_py_nested_item_list! because `Item<Source>` does not implement
// `Display`.  It is defined manually below.
#[pyclass(str, eq)]
#[derive(Clone, PartialEq, Deserialize, Serialize)]
pub struct PyNestedItemListSource {
    pub(crate) inner: NestedItemList<Source>,
}

#[pymethods]
impl PyNestedItemListSource {
    #[new]
    pub fn new(items: Vec<PyItemSource>) -> Self {
        let items: Vec<Item<Source>> = items.into_iter().map(|item| item.inner).collect();
        PyNestedItemListSource {
            inner: NestedItemList::new(items),
        }
    }

    pub fn __getitem__(&self, index: usize) -> PyResult<PyItemSource> {
        Ok(PyItemSource::from(
            self.inner.as_slice().get(index).cloned().unwrap(),
        ))
    }

    pub fn __len__(&self) -> usize {
        self.inner.len()
    }

    pub fn __iter__(slf: pyo3::pycell::PyRef<'_, Self>, py: Python) -> pyo3::PyResult<Py<PyList>> {
        let py_list = PyList::empty(py);
        for item in slf.inner.iter() {
            let py_item = PyItemSource::from(item.clone());
            py_list.append(py_item)?;
        }
        Ok(py_list.into())
    }
}

impl From<NestedItemList<Source>> for PyNestedItemListSource {
    fn from(inner: NestedItemList<Source>) -> Self {
        PyNestedItemListSource { inner }
    }
}

impl Display for PyNestedItemListSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{} source item(s)]", self.inner.len())
    }
}

macro_rules! create_conditional_interface {
    ($name: ident, $type: ident, $py_type: ident) => {
        paste::paste! {
            #[pyclass(str, get_all, set_all)]
            #[derive(Clone, Deserialize, Serialize)]
            pub struct $name {
                #[serde(rename = "if")]
                pub condition: String,
                pub then: Py<[<PyNestedItemList $type>]>,
                #[serde(rename = "else")]
                pub else_value: Option<Py<[<PyNestedItemList $type>]>>,
            }

            #[pymethods]
            impl $name {
                #[new]
                pub fn new(
                    py: Python,
                    condition: String,
                    then_value: [<PyListOrItem $type>],
                    else_value: Option<[<PyListOrItem $type>]>,
                ) -> Self {
                    // Convert ListOrItem (raw values) → NestedItemList (Item-wrapped values)
                    let then_nested = [<PyNestedItemList $type>] {
                        inner: NestedItemList::new(
                            then_value.inner.as_slice().iter()
                                .map(|v| Item::Value(Value::new_concrete(v.clone(), None)))
                                .collect()
                        ),
                    };
                    let else_nested = else_value.map(|ev| [<PyNestedItemList $type>] {
                        inner: NestedItemList::new(
                            ev.inner.as_slice().iter()
                                .map(|v| Item::Value(Value::new_concrete(v.clone(), None)))
                                .collect()
                        ),
                    });
                    Self {
                        condition,
                        then: Py::new(py, then_nested).unwrap(),
                        else_value: else_nested.map(|v| Py::new(py, v).unwrap()),
                    }
                }

                #[getter]
                pub fn condition(&self) -> String {
                    self.condition.clone()
                }

                #[getter]
                pub fn then_value(&self) -> Py<[<PyNestedItemList $type>]> {
                    self.then.clone()
                }

                pub fn __eq__(&self, py: Python, other: &Self) -> bool {

                    let then_value = self.then.borrow(py).clone();
                    let other_then_value = other.then.borrow(py).clone();

                    let else_match = match (&self.else_value, &other.else_value) {
                        (Some(a), Some(b)) => a.borrow(py).clone() == b.borrow(py).clone(),
                        (None, None) => true,
                        _ => false,
                    };

                    self.condition == other.condition
                        && then_value == other_then_value
                        && else_match
                }

            }

            impl $name {
                pub fn from_conditional(
                    py: Python,
                    conditional: Conditional<$type>,
                ) -> Self {
                    let py_then: [<PyNestedItemList $type>] = conditional.then.clone().into();
                    let py_else: Option<[<PyNestedItemList $type>]> = conditional.else_value.clone().map(|v| v.into());

                    let then_value = Py::new(py, py_then).unwrap();
                    let else_value = py_else.map(|v| Py::new(py, v).unwrap());

                    Self {
                        condition: conditional.condition.to_string(),
                        then: then_value,
                        else_value,
                    }
                }

                pub fn into_conditional(
                    py: Python,
                    conditional: $name,
                ) -> Conditional<$type> {
                    let then_nested = conditional.then.borrow(py).clone().inner;
                    let else_nested = conditional.else_value.as_ref().map(|v| v.borrow(py).clone().inner);

                    let condition = JinjaExpression::new(conditional.condition.clone())
                        .unwrap_or_else(|_| {
                            // If it fails to parse as a valid expression, create a
                            // simple true expression as fallback (should not happen
                            // in practice).
                            JinjaExpression::new("true".to_string()).unwrap()
                        });

                    Conditional::<$type> {
                        condition,
                        then: then_nested,
                        else_value: else_nested,
                        condition_span: None,
                    }
                }
            }

            impl Display for $name {
                fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                    write!(f, "{}", self.condition)
                }
            }
        }
    };
}

create_conditional_interface!(PyConditionalString, String, PyWrapString);
create_conditional_interface!(
    PyConditionalPackageDependency,
    PackageDependency,
    PyPackageDependency
);

create_conditional_interface!(PyConditionalSource, Source, PySource);

impl<'a> TryFrom<Bound<'a, PyAny>> for PyItemPackageDependency {
    type Error = PyErr;
    fn try_from(value: Bound<'a, PyAny>) -> Result<Self, Self::Error> {
        let intern_val = intern!(value.py(), "_inner");
        if !value.hasattr(intern_val)? {
            return Err(PyTypeError::new_err(
                "object is not a PackageDependency type",
            ));
        }

        let inner = value.getattr(intern_val)?;
        if !inner.is_instance_of::<Self>() {
            return Err(PyTypeError::new_err("'_inner' is invalid"));
        }

        PyItemPackageDependency::extract_bound(&inner)
    }
}

impl From<PyListOrItemString> for ListOrItem<String> {
    fn from(py_list_or_item: PyListOrItemString) -> Self {
        py_list_or_item.inner
    }
}
