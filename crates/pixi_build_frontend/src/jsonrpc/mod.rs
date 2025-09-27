use jsonrpsee::core::traits::ToRpcParams;
use serde::Serialize;
use serde_json::value::RawValue;

mod stdio;
pub(crate) use stdio::stdio_transport;

/// A helper struct to convert a serializable type into a JSON-RPC parameter.
pub struct RpcParams<T>(pub T);

impl<T: Serialize> ToRpcParams for RpcParams<T> {
    fn to_rpc_params(self) -> Result<Option<Box<RawValue>>, serde_json::Error> {
        let json = serde_json::to_string(&self.0)?;
        RawValue::from_string(json).map(Some)
    }
}

impl<T> From<T> for RpcParams<T> {
    fn from(value: T) -> Self {
        Self(value)
    }
}
