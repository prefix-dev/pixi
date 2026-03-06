use std::fmt::{Display, Formatter};

use crate::{create_py_wrap, recipe_stage0::conditional::PyItemPackageDependency};

use pyo3::{prelude::*, types::PyList};
use recipe_stage0::{matchspec::PackageDependency, recipe::Item};
use serde::{Deserialize, Serialize};

#[derive(Clone, Default, Deserialize, Serialize)]
#[pyclass(str)]
pub(crate) struct PyVecItemPackageDependency {
    pub(crate) inner: Vec<Item<PackageDependency>>,
}

impl Display for PyVecItemPackageDependency {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "[")?;
        for item in &self.inner {
            write!(f, "{item}, ")?;
        }
        write!(f, "]")
    }
}

impl From<Vec<Item<PackageDependency>>> for PyVecItemPackageDependency {
    fn from(inner: Vec<Item<PackageDependency>>) -> Self {
        PyVecItemPackageDependency { inner }
    }
}

#[pymethods]
impl PyVecItemPackageDependency {
    #[new]
    pub fn new() -> Self {
        PyVecItemPackageDependency { inner: Vec::new() }
    }

    // Implementing the PyList interface
    pub fn __getitem__(&self, index: usize) -> PyResult<PyItemPackageDependency> {
        Ok(Into::<PyItemPackageDependency>::into(
            self.inner.get(index).cloned().unwrap(),
        ))
    }

    pub fn __len__(&self) -> usize {
        self.inner.len()
    }

    pub fn __contains__(&self, item: PyItemPackageDependency) -> bool {
        self.inner.contains(&item.inner)
    }

    pub fn append(&mut self, item: PyItemPackageDependency) {
        self.inner.push(item.inner);
    }

    pub fn extend(&mut self, items: &Bound<'_, PyList>) {
        let items: Vec<PyItemPackageDependency> = items
            .iter()
            .map(|item| item.extract::<PyItemPackageDependency>().unwrap())
            .collect();

        let rust_items = items
            .into_iter()
            .map(|item| item.inner)
            .collect::<Vec<Item<PackageDependency>>>();

        self.inner.extend(rust_items);
    }

    pub fn insert(&mut self, index: usize, item: PyItemPackageDependency) {
        self.inner.insert(index, item.inner);
    }

    pub fn remove(&mut self, item: PyItemPackageDependency) -> PyResult<()> {
        let target_item = item.clone().inner;
        if let Some(pos) = self.inner.iter().position(|x| *x == target_item) {
            self.inner.remove(pos);
            Ok(())
        } else {
            Err(pyo3::exceptions::PyValueError::new_err("item not found"))
        }
    }

    pub fn pop(&mut self, index: Option<isize>) -> PyResult<PyItemPackageDependency> {
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

        let removed = self.inner.remove(idx);
        Ok(Into::<PyItemPackageDependency>::into(removed))
    }

    pub fn index(
        &self,
        item: PyItemPackageDependency,
        start: Option<usize>,
        end: Option<usize>,
    ) -> PyResult<usize> {
        let start = start.unwrap_or(0);
        let end = end.unwrap_or(self.inner.len());

        if start >= self.inner.len() {
            return Err(pyo3::exceptions::PyValueError::new_err("item not found"));
        }

        for (i, existing_item) in self.inner[start..end.min(self.inner.len())]
            .iter()
            .enumerate()
        {
            let target_item = item.clone().inner;
            if *existing_item == target_item {
                return Ok(start + i);
            }
        }

        Err(pyo3::exceptions::PyValueError::new_err("item not found"))
    }

    pub fn count(&self, item: PyItemPackageDependency) -> usize {
        let target_item = item.clone().inner;
        self.inner.iter().filter(|&x| *x == target_item).count()
    }

    pub fn clear(&mut self) {
        self.inner.clear();
    }

    pub fn reverse(&mut self) {
        self.inner.reverse();
    }

    pub fn copy(&self) -> Self {
        self.clone()
    }

    pub fn __setitem__(&mut self, index: usize, value: PyItemPackageDependency) -> PyResult<()> {
        if index >= self.inner.len() {
            return Err(pyo3::exceptions::PyIndexError::new_err(
                "list index out of range",
            ));
        }
        self.inner[index] = value.inner;
        Ok(())
    }

    pub fn __delitem__(&mut self, index: usize) -> PyResult<()> {
        if index >= self.inner.len() {
            return Err(pyo3::exceptions::PyIndexError::new_err(
                "list index out of range",
            ));
        }
        self.inner.remove(index);
        Ok(())
    }

    pub fn __iter__(slf: pyo3::pycell::PyRef<'_, Self>, py: Python) -> pyo3::PyResult<Py<PyList>> {
        let py_list = PyList::empty(py);
        for item in slf.inner.iter() {
            let py_item: PyItemPackageDependency = item.clone().into();
            py_list.append(py_item)?;
        }
        Ok(py_list.into())
    }
}

create_py_wrap!(
    _PyVecItemPackageDependency,
    Vec<Item<PackageDependency>>,
    |vec: &Vec<Item<PackageDependency>>, f: &mut Formatter<'_>| {
        write!(f, "[")?;
        for item in vec {
            write!(f, "{item}, ")?;
        }
        write!(f, "]")
    }
);
