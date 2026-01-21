use ::serde::{Deserialize, Serialize};
use pyo3::exceptions::PyTypeError;
use pyo3::types::{PyAnyMethods, PyList, PyListMethods};
use pyo3::{Bound, FromPyObject, Py, PyAny, PyErr, PyResult, Python, intern, pyclass, pymethods};
use recipe_stage0::matchspec::PackageDependency;
use recipe_stage0::recipe::{Conditional, Item, ListOrItem, Source, Value};
use std::fmt::Display;

use std::ops::Deref;

use crate::create_py_wrap;
use crate::recipe_stage0::recipe::PySource;
use crate::recipe_stage0::requirements::PyPackageDependency;

/// Creates a PyItem class for a given type.
/// The first argument is the name of the class, the second
/// is the type it wraps, and the third is the Python type.
/// It is necessary to provide the Python type because
/// the String equivalent is still String
/// but for other types it will be some type
/// prefixed with Py, like PyPackageDependency.
macro_rules! create_py_item {
    ($name: ident, $type: ident, $py_type: ident) => {
        paste::paste! {
            #[pyclass]
            #[derive(Clone, Serialize, Deserialize)]
            pub struct $name {
                pub(crate) inner: Item<$type>,
            }

            #[pymethods]
            impl $name {
                #[new]
                pub fn new(value: String) -> PyResult<Self> {
                    let item: Item<_> = value
                        .parse()
                        .map_err(|_| PyTypeError::new_err(format!("Failed to parse {value}")))?;

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
                    matches!(self.inner, Item::Value(Value::Template(_)))
                }

                pub fn is_conditional(&self) -> bool {
                    matches!(self.inner, Item::Conditional(_))
                }

                pub fn __str__(&self) -> String {
                    self.inner.to_string()
                }

                pub fn concrete(&self) -> Option<$py_type> {
                    if let Item::Value(Value::Concrete(val)) = &self.inner {
                        Some(val.clone().into())
                    } else {
                        None
                    }
                }

                pub fn template(&self) -> Option<String> {
                    if let Item::Value(Value::Template(val)) = &self.inner {
                        Some(val.clone())
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

create_py_item!(
    PyItemPackageDependency,
    PackageDependency,
    PyPackageDependency
);
create_py_item!(PyItemString, String, String);
create_py_item!(PyItemSource, Source, PySource);

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
                    inner: ListOrItem::<$type>(item),
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
                    self.inner.0.get(index).cloned().unwrap(),
                ))
            }

            pub fn __len__(&self) -> usize {
                self.inner.0.len()
            }

            pub fn __contains__(&self, item: $py_type) -> bool {
                let inner_item: $type = item.into();
                self.inner.0.contains(&inner_item)
            }

            pub fn append(&mut self, item: $py_type) {
                self.inner.0.push(item.into());
            }

            pub fn extend(&mut self, items: &Bound<'_, PyList>) {
                let items: Vec<$type> = items
                    .iter()
                    .map(|item| item.extract::<$py_type>().unwrap().into())
                    .collect();
                self.inner.0.extend(items);
            }

            pub fn insert(&mut self, index: usize, item: $py_type) {
                self.inner.0.insert(index, item.into());
            }

            pub fn remove(&mut self, item: $py_type) -> PyResult<()> {
                let target_item: $type = item.clone().into();
                if let Some(pos) = self.inner.0.iter().position(|x| *x == target_item) {
                    self.inner.0.remove(pos);
                    Ok(())
                } else {
                    Err(pyo3::exceptions::PyValueError::new_err("item not found"))
                }
            }

            pub fn pop(&mut self, index: Option<isize>) -> PyResult<$py_type> {
                let len = self.inner.0.len();
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

                let removed = self.inner.0.remove(idx);
                Ok(Into::<$py_type>::into(removed))
            }

            pub fn index(
                &self,
                item: $py_type,
                start: Option<usize>,
                end: Option<usize>,
            ) -> PyResult<usize> {
                let start = start.unwrap_or(0);
                let end = end.unwrap_or(self.inner.0.len());

                if start >= self.inner.0.len() {
                    return Err(pyo3::exceptions::PyValueError::new_err("item not found"));
                }

                for (i, existing_item) in self.inner.0[start..end.min(self.inner.0.len())]
                    .iter()
                    .enumerate()
                {
                    let target_item: $type = item.clone().into();
                    if *existing_item == target_item {
                        return Ok(start + i);
                    }
                }

                Err(pyo3::exceptions::PyValueError::new_err("item not found"))
            }

            pub fn count(&self, item: $py_type) -> usize {
                let target_item: $type = item.clone().into();
                self.inner.0.iter().filter(|&x| *x == target_item).count()
            }

            pub fn clear(&mut self) {
                self.inner.0.clear();
            }

            pub fn reverse(&mut self) {
                self.inner.0.reverse();
            }

            pub fn copy(&self) -> Self {
                self.clone()
            }

            pub fn __setitem__(&mut self, index: usize, value: $py_type) -> PyResult<()> {
                if index >= self.inner.0.len() {
                    return Err(pyo3::exceptions::PyIndexError::new_err(
                        "list index out of range",
                    ));
                }
                self.inner.0[index] = value.into();
                Ok(())
            }

            pub fn __delitem__(&mut self, index: usize) -> PyResult<()> {
                if index >= self.inner.0.len() {
                    return Err(pyo3::exceptions::PyIndexError::new_err(
                        "list index out of range",
                    ));
                }
                self.inner.0.remove(index);
                Ok(())
            }

            pub fn __iter__(
                slf: pyo3::pycell::PyRef<'_, Self>,
                py: Python,
            ) -> pyo3::PyResult<Py<PyList>> {
                let py_list = PyList::empty(py);
                for item in slf.inner.0.iter() {
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
                write!(f, "{}", self.inner)
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
create_pylist_or_item!(PyListOrItemSource, Source, PySource);

create_py_wrap!(PyWrapString, String);

macro_rules! create_conditional_interface {
    ($name: ident, $type: ident, $py_type: ident) => {
        paste::paste! {
            #[pyclass(str, get_all, set_all)]
            #[derive(Clone, Deserialize, Serialize)]
            pub struct $name {
                #[serde(rename = "if")]
                pub condition: String,
                pub then: Py<[<PyListOrItem $type>]>,
                #[serde(rename = "else")]
                pub else_value: Py<[<PyListOrItem $type>]>,
            }

            #[pymethods]
            impl $name {
                #[new]
                pub fn new(
                    py: Python,
                    condition: String,
                    then_value: [<PyListOrItem $type>],
                    else_value: [<PyListOrItem $type>],
                ) -> Self {
                    Self {
                        condition,
                        then: Py::new(py, then_value).unwrap(),
                        else_value: Py::new(py, else_value).unwrap(),
                    }
                }

                #[getter]
                pub fn condition(&self) -> String {
                    self.condition.clone()
                }

                #[getter]
                pub fn then_value(&self) -> Py<[<PyListOrItem $type>]> {
                    self.then.clone()
                }

                pub fn __eq__(&self, py: Python, other: &Self) -> bool {

                    let then_value = self.then.borrow(py).clone();
                    let other_then_value = other.then.borrow(py).clone();

                    let else_value = self.else_value.borrow(py).clone();
                    let other_else_value = other.else_value.borrow(py).clone();

                    self.condition == other.condition
                        && then_value == other_then_value
                        && else_value == other_else_value
                }

            }

            impl $name {
                pub fn from_conditional(
                    py: Python,
                    conditional: Conditional<$type>,
                ) -> Self {
                    let py_list_or_item: [<PyListOrItem $type>] = conditional.then.clone().into();
                    let py_list_or_item_else: [<PyListOrItem $type>] = conditional.else_value.clone().into();

                    let then_value = Py::new(py, py_list_or_item).unwrap();
                    let else_value = Py::new(py, py_list_or_item_else).unwrap();

                    Self {
                        condition: conditional.condition,
                        then: then_value,
                        else_value,
                    }
                }

                pub fn into_conditional(
                    py: Python,
                    conditional: $name,
                ) -> Conditional<$type> {
                    let then_list_or_item = conditional.then.borrow(py).clone().inner;
                    let else_value_list_or_item = conditional.else_value.borrow(py).clone().inner;

                    let condition = conditional.condition.clone();

                    Conditional::<$type> {
                        condition,
                        then: then_list_or_item,
                        else_value: else_value_list_or_item,
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
