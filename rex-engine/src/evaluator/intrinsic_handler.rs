use std::sync::Arc;

use blake3::Hash;
use chrono::{DateTime, Utc};
use rex_ast::Symbol;
use rex_typesystem::types::{RexType, Scheme, Type};
use uuid::Uuid;

use crate::{
    EngineError,
    evaluator::native_callable::{SchedulerNativeCallable, SchedulerNativeResult},
    memory::heap::{RootScope, RootedPtr},
};

pub(crate) trait InternalFnSync<State, Sig>: Send + Sync + 'static
where
    State: Clone + Send + Sync + 'static,
{
    fn into_registration(
        self,
        state: Arc<State>,
        name: Symbol,
    ) -> (Scheme, usize, SchedulerNativeCallable);
}

trait FromInternal: RexType + Sized {
    fn from_internal(scope: &mut RootScope<'_>, value: RootedPtr) -> Result<Self, EngineError>;
}

trait IntoInternal: RexType {
    fn into_internal(self, scope: &mut RootScope<'_>) -> Result<RootedPtr, EngineError>;
}

macro_rules! impl_internal_scalar {
    ($ty:ty, $read:ident, $write:ident) => {
        impl FromInternal for $ty {
            fn from_internal(
                scope: &mut RootScope<'_>,
                value: RootedPtr,
            ) -> Result<Self, EngineError> {
                scope.$read(value)
            }
        }

        impl IntoInternal for $ty {
            fn into_internal(self, scope: &mut RootScope<'_>) -> Result<RootedPtr, EngineError> {
                scope.$write(self)
            }
        }
    };
}

impl_internal_scalar!(bool, root_as_bool, alloc_root_bool);
impl_internal_scalar!(u8, root_as_u8, alloc_root_u8);
impl_internal_scalar!(u16, root_as_u16, alloc_root_u16);
impl_internal_scalar!(u32, root_as_u32, alloc_root_u32);
impl_internal_scalar!(u64, root_as_u64, alloc_root_u64);
impl_internal_scalar!(i8, root_as_i8, alloc_root_i8);
impl_internal_scalar!(i16, root_as_i16, alloc_root_i16);
impl_internal_scalar!(i32, root_as_i32, alloc_root_i32);
impl_internal_scalar!(i64, root_as_i64, alloc_root_i64);
impl_internal_scalar!(f32, root_as_f32, alloc_root_f32);
impl_internal_scalar!(f64, root_as_f64, alloc_root_f64);
impl_internal_scalar!(String, root_as_string, alloc_root_string);
impl_internal_scalar!(Uuid, root_as_uuid, alloc_root_uuid);
impl_internal_scalar!(Hash, root_as_hash, alloc_root_hash);
impl_internal_scalar!(DateTime<Utc>, root_as_datetime, alloc_root_datetime);

fn function_type(arguments: impl IntoIterator<Item = Type>, result: Type) -> Type {
    arguments
        .into_iter()
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .fold(result, |result, argument| Type::fun(argument, result))
}

macro_rules! define_internal_handler {
    ([] ; $arity:literal ; $sig:ty) => {
        impl<State, F, R> InternalFnSync<State, $sig> for F
        where
            State: Clone + Send + Sync + 'static,
            F: for<'a> Fn(&'a State) -> Result<R, EngineError> + Send + Sync + 'static,
            R: IntoInternal,
        {
            fn into_registration(
                self,
                state: Arc<State>,
                name: Symbol,
            ) -> (Scheme, usize, SchedulerNativeCallable) {
                let typ = R::rex_type();
                let callable: SchedulerNativeCallable = Arc::new(move |scope, _typ, args| {
                    if !args.is_empty() {
                        return Err(EngineError::NativeArity {
                            name: name.clone(),
                            expected: $arity,
                            got: args.len(),
                        });
                    }
                    let result = self(state.as_ref())?;
                    Ok(SchedulerNativeResult::Ready(result.into_internal(scope)?))
                });
                (Scheme::new(vec![], vec![], typ), $arity, callable)
            }
        }
    };
    ([ $(($arg_ty:ident, $arg_name:ident, $idx:tt)),+ ] ; $arity:literal ; $sig:ty) => {
        impl<State, F, R, $($arg_ty),+> InternalFnSync<State, $sig> for F
        where
            State: Clone + Send + Sync + 'static,
            F: for<'a> Fn(&'a State, $($arg_ty),+) -> Result<R, EngineError>
                + Send
                + Sync
                + 'static,
            R: IntoInternal,
            $($arg_ty: FromInternal),+
        {
            fn into_registration(
                self,
                state: Arc<State>,
                name: Symbol,
            ) -> (Scheme, usize, SchedulerNativeCallable) {
                let typ = function_type([$($arg_ty::rex_type()),+], R::rex_type());
                let callable: SchedulerNativeCallable = Arc::new(move |scope, _typ, args| {
                    if args.len() != $arity {
                        return Err(EngineError::NativeArity {
                            name: name.clone(),
                            expected: $arity,
                            got: args.len(),
                        });
                    }
                    $(let $arg_name = $arg_ty::from_internal(scope, args[$idx])?;)+
                    let result = self(state.as_ref(), $($arg_name),+)?;
                    Ok(SchedulerNativeResult::Ready(result.into_internal(scope)?))
                });
                (Scheme::new(vec![], vec![], typ), $arity, callable)
            }
        }
    };
}

define_internal_handler!([] ; 0 ; fn() -> R);
define_internal_handler!([(A, a, 0)] ; 1 ; fn(A) -> R);
define_internal_handler!([(A, a, 0), (B, b, 1)] ; 2 ; fn(A, B) -> R);
