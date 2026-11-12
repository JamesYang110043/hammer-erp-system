//! 金額、數量的 JSON 序列化（`docs/architecture.md` D9）。
//!
//! 所有 [`Decimal`] 在 JSON 中一律是**字串**（例如 `"12.3400"`），不可以是 JSON number，
//! 以免經過 JavaScript 或其他 JSON 解析器時被轉成浮點數而失真。
//! 序列化時保留原本的小數位數（scale）。
//!
//! 反序列化只接受字串；收到 JSON number 會回傳錯誤。
//!
//! 用法（`specta(type = String)` 讓產生的 TypeScript 型別也是 `string`）：
//!
//! ```
//! use hammer_shared::decimal::Decimal;
//!
//! #[derive(serde::Serialize, serde::Deserialize, specta::Type)]
//! struct Line {
//!     #[serde(with = "hammer_shared::decimal::serde_str")]
//!     #[specta(type = String)]
//!     quantity: Decimal,
//!
//!     #[serde(with = "hammer_shared::decimal::serde_str_option", default)]
//!     #[specta(type = Option<String>)]
//!     unit_price: Option<Decimal>,
//! }
//! ```

pub use rust_decimal::Decimal;

/// `Decimal` ⇄ JSON 字串。搭配 `#[serde(with = "hammer_shared::decimal::serde_str")]`。
pub mod serde_str {
    use super::Decimal;
    use serde::{Deserialize, Deserializer, Serializer, de::Error as _};
    use std::str::FromStr;

    pub fn serialize<S: Serializer>(value: &Decimal, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(value)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Decimal, D::Error> {
        let s = <std::borrow::Cow<'de, str>>::deserialize(deserializer)?;
        Decimal::from_str(s.trim()).map_err(D::Error::custom)
    }
}

/// `Option<Decimal>` ⇄ JSON 字串或 `null`。
/// 搭配 `#[serde(with = "hammer_shared::decimal::serde_str_option", default)]`。
pub mod serde_str_option {
    use super::Decimal;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(
        value: &Option<Decimal>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match value {
            Some(v) => serializer.collect_str(v),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<Decimal>, D::Error> {
        #[derive(Deserialize)]
        struct Wrapper(#[serde(with = "super::serde_str")] Decimal);

        Ok(Option::<Wrapper>::deserialize(deserializer)?.map(|Wrapper(v)| v))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};
    use serde_json::json;
    use std::str::FromStr;

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Amount {
        #[serde(with = "serde_str")]
        value: Decimal,
        #[serde(with = "serde_str_option", default)]
        optional: Option<Decimal>,
    }

    fn dec(s: &str) -> Decimal {
        Decimal::from_str(s).unwrap()
    }

    #[test]
    fn serializes_as_json_string_keeping_scale() {
        let a = Amount {
            value: dec("12.3400"),
            optional: None,
        };
        assert_eq!(
            serde_json::to_value(&a).unwrap(),
            json!({ "value": "12.3400", "optional": null })
        );
    }

    #[test]
    fn point_one_plus_point_two_round_trips_exactly() {
        let sum = dec("0.1") + dec("0.2");
        assert_eq!(sum, dec("0.3"));

        let a = Amount {
            value: sum,
            optional: Some(sum),
        };
        let text = serde_json::to_string(&a).unwrap();
        assert_eq!(text, r#"{"value":"0.3","optional":"0.3"}"#);

        let back: Amount = serde_json::from_str(&text).unwrap();
        assert_eq!(back, a);
        assert_eq!(back.value.to_string(), "0.3");
    }

    #[test]
    fn round_trips_large_and_high_precision_values() {
        for s in [
            "0",
            "-0.0001",
            "99999999999999.9999", // NUMERIC(18,4) 上限
            "1234567.123456",      // 匯率 NUMERIC(18,6)
            "0.1000",
            "79228162514264337593543950335", // Decimal 最大值
        ] {
            let a = Amount {
                value: dec(s),
                optional: Some(dec(s)),
            };
            let text = serde_json::to_string(&a).unwrap();
            let back: Amount = serde_json::from_str(&text).unwrap();
            assert_eq!(back, a);
            assert_eq!(back.value.to_string(), s, "精度或小數位數改變");
        }
    }

    #[test]
    fn rejects_json_number() {
        let err = serde_json::from_value::<Amount>(json!({ "value": 0.3 }));
        assert!(err.is_err());
        let err = serde_json::from_value::<Amount>(json!({ "value": "1", "optional": 0.3 }));
        assert!(err.is_err());
    }

    #[test]
    fn rejects_invalid_string() {
        assert!(serde_json::from_value::<Amount>(json!({ "value": "abc" })).is_err());
    }

    #[test]
    fn optional_accepts_missing_and_null() {
        let a: Amount = serde_json::from_value(json!({ "value": "1" })).unwrap();
        assert_eq!(a.optional, None);
        let a: Amount = serde_json::from_value(json!({ "value": "1", "optional": null })).unwrap();
        assert_eq!(a.optional, None);
    }
}
