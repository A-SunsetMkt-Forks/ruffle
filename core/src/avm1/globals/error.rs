//! Error object

use crate::avm1::activation::Activation;
use crate::avm1::error::Error;
use crate::avm1::property_decl::{define_properties_on, Declaration};
use crate::avm1::{Object, Value};
use crate::string::StringContext;
use ruffle_macros::istr;

const PROTO_DECLS: &[Declaration] = declare_properties! {
    "message" => string("Error");
    "name" => string("Error");
    "toString" => method(to_string);
};

pub fn constructor<'gc>(
    activation: &mut Activation<'_, 'gc>,
    this: Object<'gc>,
    args: &[Value<'gc>],
) -> Result<Value<'gc>, Error<'gc>> {
    let message: Value<'gc> = args.get(0).cloned().unwrap_or(Value::Undefined);

    if message != Value::Undefined {
        this.set(istr!("message"), message, activation)?;
    }

    Ok(Value::Undefined)
}

pub fn create_proto<'gc>(
    context: &mut StringContext<'gc>,
    proto: Object<'gc>,
    fn_proto: Object<'gc>,
) -> Object<'gc> {
    let object = Object::new(context, Some(proto));
    define_properties_on(PROTO_DECLS, context, object, fn_proto);
    object
}

fn to_string<'gc>(
    activation: &mut Activation<'_, 'gc>,
    this: Object<'gc>,
    _args: &[Value<'gc>],
) -> Result<Value<'gc>, Error<'gc>> {
    let message = this.get(istr!("message"), activation)?;
    Ok(message.coerce_to_string(activation)?.into())
}
