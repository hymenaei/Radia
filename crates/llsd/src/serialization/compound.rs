use std::marker::PhantomData;

use serde::ser::{
    Serialize, SerializeMap, SerializeSeq, SerializeStruct, SerializeStructVariant, SerializeTuple, SerializeTupleStruct,
    SerializeTupleVariant,
};

use crate::{Error, Result};

use super::{Container, Format, Serializer, key::KeySerializer};

pub(super) struct Compound<'a, F: Format> {
    pub(super) serializer: &'a mut Serializer<F>,
    pub(super) kind: Container,
    pub(super) offset: usize,
    pub(super) count: usize,
    pub(super) pending_key: bool,
    pub(super) outer_map: Option<usize>,
}

impl<F: Format> Compound<'_, F> {
    fn separator(&mut self) -> Result<()> {
        F::separator(&mut self.serializer.output, self.count)
    }

    fn element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<()> {
        self.separator()?;
        value.serialize(&mut *self.serializer)?;
        self.count = self.count.checked_add(1).ok_or(Error::LimitExceeded("encoded length"))?;
        Ok(())
    }

    fn finish(self) -> Result<()> {
        if self.pending_key {
            return Err(Error::InvalidValue("map key has no value".into()));
        }
        F::close(&mut self.serializer.output, self.kind, self.offset, self.count)?;
        if let Some(offset) = self.outer_map {
            F::close(&mut self.serializer.output, Container::Map, offset, 1)?;
        }
        Ok(())
    }
}

impl<F: Format> Drop for Compound<'_, F> {
    fn drop(&mut self) {
        self.serializer.depth -= 1 + usize::from(self.outer_map.is_some());
    }
}

impl<F: Format> SerializeSeq for Compound<'_, F> {
    type Ok = ();
    type Error = Error;

    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<()> {
        self.element(value)
    }

    fn end(self) -> Result<()> {
        self.finish()
    }
}

impl<F: Format> SerializeTuple for Compound<'_, F> {
    type Ok = ();
    type Error = Error;

    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<()> {
        self.element(value)
    }

    fn end(self) -> Result<()> {
        self.finish()
    }
}

impl<F: Format> SerializeTupleStruct for Compound<'_, F> {
    type Ok = ();
    type Error = Error;

    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<()> {
        self.element(value)
    }

    fn end(self) -> Result<()> {
        self.finish()
    }
}

impl<F: Format> SerializeTupleVariant for Compound<'_, F> {
    type Ok = ();
    type Error = Error;

    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<()> {
        self.element(value)
    }

    fn end(self) -> Result<()> {
        self.finish()
    }
}

impl<F: Format> SerializeMap for Compound<'_, F> {
    type Ok = ();
    type Error = Error;

    fn serialize_key<T: Serialize + ?Sized>(&mut self, key: &T) -> Result<()> {
        if self.pending_key {
            return Err(Error::InvalidValue("map key has no value".into()));
        }
        self.separator()?;
        let key = key.serialize(KeySerializer::<F> { budget: self.serializer.output.remaining(), _format: PhantomData })?;
        F::key(&mut self.serializer.output, &key)?;
        self.pending_key = true;
        Ok(())
    }

    fn serialize_value<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<()> {
        if !self.pending_key {
            return Err(Error::InvalidValue("map value has no key".into()));
        }
        value.serialize(&mut *self.serializer)?;
        self.pending_key = false;
        self.count = self.count.checked_add(1).ok_or(Error::LimitExceeded("encoded length"))?;
        Ok(())
    }

    fn end(self) -> Result<()> {
        self.finish()
    }
}

impl<F: Format> SerializeStruct for Compound<'_, F> {
    type Ok = ();
    type Error = Error;

    fn serialize_field<T: Serialize + ?Sized>(&mut self, key: &'static str, value: &T) -> Result<()> {
        self.separator()?;
        F::key(&mut self.serializer.output, key)?;
        value.serialize(&mut *self.serializer)?;
        self.count = self.count.checked_add(1).ok_or(Error::LimitExceeded("encoded length"))?;
        Ok(())
    }

    fn end(self) -> Result<()> {
        self.finish()
    }
}

impl<F: Format> SerializeStructVariant for Compound<'_, F> {
    type Ok = ();
    type Error = Error;

    fn serialize_field<T: Serialize + ?Sized>(&mut self, key: &'static str, value: &T) -> Result<()> {
        SerializeStruct::serialize_field(self, key, value)
    }

    fn end(self) -> Result<()> {
        self.finish()
    }
}
