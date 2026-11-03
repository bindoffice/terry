//! Google's `generateContent` API: the Generative Language endpoint and Vertex
//! AI. Both share the wire format, so [`shared`] holds the transcript/tool/
//! thinking conversion and [`provider`] holds the streaming adapter.

pub mod provider;
pub mod shared;

pub use provider::{
    build_body, google_generative_ai_models, stream_url, GoogleApi, GoogleOptions, GoogleProvider,
};
