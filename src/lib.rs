use std::collections::HashMap;

use avro_rs::from_avro_datum;
use avro_rs::to_avro_datum;
use avro_rs::types::Value;
use avro_rs::Schema as SchemaRs;
use pyo3::exceptions;
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDict, PyList};
use pyo3::PyDowncastError;


#[pyclass]
struct Schema {
    schema: SchemaRs,
}

#[pymethods]
impl Schema {
    #[new]
    fn new(input: &str) -> PyResult<Self> {
        match SchemaRs::parse_str(&input) {
            Ok(schema) => Ok(Schema { schema }),
            Err(e) => Err(PyErr::new::<exceptions::ValueError, _>(format!(
                "{}",
                e.as_fail()
            ))),
        }
    }

    fn write<'p>(&self, py: Python<'p>, datum: PyObject) -> PyResult<&'p PyBytes> {
        let value = to_avro_value(py, &datum, &self.schema)?;

        match to_avro_datum(&self.schema, value) {
            Ok(bytes) => Ok(PyBytes::new(py, &bytes)),
            Err(e) => Err(PyErr::new::<exceptions::ValueError, _>(format!(
                "{}",
                e.as_fail()
            ))),
        }
    }

    fn read(&self, py: Python, datum: &PyBytes) -> PyResult<PyObject> {
        let mut bytes = datum.as_bytes();
        match from_avro_datum(&self.schema, &mut bytes, None) {
            Ok(value) => to_pyobject(py, value),
            Err(e) => Err(PyErr::new::<exceptions::ValueError, _>(format!(
                "{}",
                e.as_fail()
            ))),
        }
    }
}

fn to_pyobject(py: Python, datum: Value) -> PyResult<PyObject> {
    match datum {
        Value::Null => Ok(py.None()),
        Value::Boolean(b) => Ok(b.to_object(py)),
        Value::Int(n) => Ok(n.to_object(py)),
        Value::Long(n) => Ok(n.to_object(py)),
        Value::Float(x) => Ok(x.to_object(py)),
        Value::Double(x) => Ok(x.to_object(py)),
        Value::Bytes(bytes) => Ok(bytes.to_object(py)),
        Value::String(string) => Ok(string.to_object(py)),
        Value::Fixed(_, bytes) => Ok(bytes.to_object(py)),
        Value::Enum(_, symbol) => Ok(symbol.to_object(py)),
        Value::Union(item) => to_pyobject(py, *item),
        Value::Array(items) => {
            // TODO
            let list = PyList::empty(py);
            for item in items {
                list.append(to_pyobject(py, item)?)?;
            }
            Ok(list.to_object(py))
        }
        Value::Map(items) => {
            // TODO
            let dict = PyDict::new(py);
            for (key, value) in items {
                dict.set_item(key, to_pyobject(py, value)?)?;
            }
            Ok(dict.to_object(py))
        }
        Value::Record(fields) => {
            let dict = PyDict::new(py);
            for (name, value) in fields {
                dict.set_item(name, to_pyobject(py, value)?)?;
            }
            Ok(dict.to_object(py))
        }
        _ => Ok(py.None()),
    }
}

fn to_avro_value(py: Python, datum: &PyObject, schema: &SchemaRs) -> PyResult<Value> {
    match schema {
        &SchemaRs::Null if datum.is_none(py) => Ok(Value::Null),
        &SchemaRs::Null => Err(PyErr::from(PyDowncastError)),
        &SchemaRs::Boolean => {
            let b = datum.extract::<bool>(py)?;
            Ok(Value::Boolean(b))
        }
        &SchemaRs::Int => {
            // TODO: PyInt/PyLong?
            let n = datum.extract::<i32>(py)?;
            Ok(Value::Int(n))
        }
        &SchemaRs::Long => {
            // TODO: PyInt/PyLong?
            let n = datum.extract::<i64>(py)?;
            Ok(Value::Long(n))
        }
        &SchemaRs::Float => {
            let x = datum.extract::<f32>(py)?;
            Ok(Value::Float(x))
        }
        &SchemaRs::Double => {
            let x = datum.extract::<f64>(py)?;
            Ok(Value::Double(x))
        }
        &SchemaRs::Bytes => {
            let bytes = datum.extract::<Vec<u8>>(py)?;
            Ok(Value::Bytes(bytes))
        }
        &SchemaRs::String => {
            let string = datum.extract::<String>(py)?;
            Ok(Value::String(string))
        }
        &SchemaRs::Array(ref inner) => {
            // TODO: PyTuple?
            let array = datum.extract::<Vec<PyObject>>(py)?;
            let items = array
                .iter()
                .map(|item| to_avro_value(py, &item, inner))
                .collect::<PyResult<Vec<Value>>>()?;
            Ok(Value::Array(items))
        }
        &SchemaRs::Map(ref inner) => {
            let items = datum
                .cast_as::<PyDict>(py)?
                .iter()
                .map(|(keyo, valueo)| {
                    Ok((
                        keyo.extract::<String>()?,
                        to_avro_value(py, &valueo.to_object(py), inner)?,
                    ))
                })
                .collect::<PyResult<HashMap<String, Value>>>()?;

            Ok(Value::Map(items))
        }
        &SchemaRs::Union(ref inner) => {
            // Optimization for when union is used for optional values
            if inner.is_nullable() & &datum.is_none(py) {
                Ok(Value::Union(Box::new(Value::Null)))
            } else {
                let variants = inner.variants();
                for variant in variants {
                    let value = to_avro_value(py, datum, variant);
                    match value {
                        Ok(v) => return Ok(Value::Union(Box::new(v))),
                        _ => continue,
                    };
                }
                Err(PyErr::from(PyDowncastError))
            }
        }
        &SchemaRs::Record { ref fields, .. } => {
            let record_dict = datum.cast_as::<PyDict>(py)?;
            let mut rfields = Vec::with_capacity(fields.len());

            for field in fields.iter() {
                let keyo = field.name.clone().to_object(py);
                match record_dict.get_item(keyo) {
                    Some(value) => {
                        let value = to_avro_value(py, &value.to_object(py), &field.schema)?;
                        rfields.push((field.name.clone(), value));
                    }
                    None => return Err(PyErr::from(PyDowncastError)),
                }
            }

            Ok(Value::Record(rfields))
        }
        &SchemaRs::Enum { ref symbols, .. } => {
            let string = datum.extract::<String>(py);
            if let Ok(string) = string {
                if let Some(index) = symbols.iter().position(|ref item| item == &&string) {
                    Ok(Value::Enum(index as i32, string))
                } else {
                    return Err(PyErr::from(PyDowncastError));
                }
            } else {
                let index = datum.extract::<i32>(py)? as usize;
                if index < symbols.len() {
                    Ok(Value::Enum(index as i32, symbols[index].clone()))
                } else {
                    return Err(PyErr::from(PyDowncastError));
                }
            }
        }
        &SchemaRs::Fixed { .. } => {
            let bytes = datum.extract::<Vec<u8>>(py)?;
            Ok(Value::Fixed(bytes.len(), bytes))
        }
        _ => Ok(Value::Null),
    }
}

#[pymodule]
fn pyo3avro_rs(_py: Python, m: &PyModule) -> PyResult<()> {
    m.add_class::<Schema>()?;
    Ok(())
}