//! Message transformation for NATS Gateway

use es_gateway_core::prelude::*;

/// Message transformer trait
pub trait MessageTransformer: Send + Sync {
    fn transform(&self, envelope: Envelope) -> Result<Envelope>;
    fn name(&self) -> &'static str;
}

/// No-op transformer
pub struct NoOpTransformer;

impl MessageTransformer for NoOpTransformer {
    fn transform(&self, envelope: Envelope) -> Result<Envelope> {
        Ok(envelope)
    }
    fn name(&self) -> &'static str { "noop" }
}

/// Field renaming transformer
pub struct FieldRenameTransformer {
    renames: Vec<(String, String)>,
}

impl FieldRenameTransformer {
    pub fn new(renames: Vec<(String, String)>) -> Self {
        Self { renames }
    }
}

impl MessageTransformer for FieldRenameTransformer {
    fn transform(&self, mut envelope: Envelope) -> Result<Envelope> {
        if let Some(ref mut source) = envelope.event.source {
            if let Some(obj) = source.as_object_mut() {
                for (from, to) in &self.renames {
                    if let Some(value) = obj.remove(from) {
                        obj.insert(to.clone(), value);
                    }
                }
            }
        }
        Ok(envelope)
    }
    fn name(&self) -> &'static str { "field_rename" }
}
