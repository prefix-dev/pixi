use itertools::Itertools;
use miette::{miette, IntoDiagnostic};
use serde::de::DeserializeOwned;
use serde::Serialize;

pub fn from_key_value_str<T>(data: &str) -> miette::Result<T>
where
    T: DeserializeOwned,
{
    use serde_json::{Map, Value};

    let value = data
        .split(',')
        .map(|kv| -> miette::Result<(String, Value)> {
            let (key, value) = kv
                .split_once('=')
                .ok_or_else(|| miette!("Invalid key-value string: {}", kv))?;

            let key = key.to_string();
            let value = match value.parse::<u64>() {
                Ok(number_value) => Value::Number(number_value.into()),
                Err(_) => Value::String(value.to_string()),
            };

            Ok((key, value))
        })
        .collect::<miette::Result<Map<String, Value>>>()?;

    let value = Value::Object(value);
    let value = serde_json::from_value(value).into_diagnostic()?;

    Ok(value)
}

pub fn to_key_value_str<T>(data: T) -> miette::Result<String>
where
    T: Serialize,
{
    let value = serde_json::to_value(data).into_diagnostic()?;
    let value = match value {
        serde_json::Value::Object(map) => map,
        _ => return Err(miette!("Expected an object")),
    };

    let value = value
        .into_iter()
        .sorted_by(|(k1, _), (k2, _)| k1.cmp(k2))
        .filter_map(|(key, value)| {
            let value = match value {
                serde_json::Value::Number(number) => number.to_string(),
                serde_json::Value::String(string) => string,
                serde_json::Value::Null => return None,
                _ => return Some(Err(miette!("Expected a string or number"))),
            };

            Some(Ok(format!("{}={}", key, value)))
        })
        .collect::<miette::Result<Vec<String>>>()?
        .join(",");

    Ok(value)
}

#[cfg(test)]
mod tests {
    use crate::utils::serde_key_value::{from_key_value_str, to_key_value_str};
    use insta::{assert_display_snapshot, assert_yaml_snapshot};
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Serialize, Deserialize)]
    struct A {
        pub a: String,
        pub b: usize,
        pub c: Option<String>,
    }

    #[test]
    fn test_de_from_kv_string() -> miette::Result<()> {
        let input = "a=value,b=123,c=optional_value";
        let result: A = from_key_value_str(input)?;

        assert_yaml_snapshot!(result, @r###"
        ---
        a: value
        b: 123
        c: optional_value
        "###);

        let input = "a=value,b=123";
        let result: A = from_key_value_str(input)?;

        assert_yaml_snapshot!(result, @r###"
        ---
        a: value
        b: 123
        c: ~
        "###);

        Ok(())
    }

    #[test]
    fn test_ser_to_kv_string() -> miette::Result<()> {
        let input = A {
            a: "value".to_string(),
            b: 123,
            c: Some("optional_value".to_string()),
        };

        let result = to_key_value_str(input)?;
        assert_display_snapshot!(result, @r###"a=value,b=123,c=optional_value"###);

        let input = A {
            a: "value".to_string(),
            b: 123,
            c: None,
        };

        let result = to_key_value_str(input)?;
        assert_display_snapshot!(result, @r###"a=value,b=123"###);

        Ok(())
    }
}
