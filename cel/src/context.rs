use crate::common::value::Val;
use crate::magic::{Function, FunctionRegistry, IntoFunction};
use crate::objects::{TryIntoValue, Value};
use crate::parser::Expression;
use crate::{Env, ExecutionError};
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

/// An immutable CEL value that can be shared cheaply between contexts.
///
/// Preparing a value performs the potentially expensive conversion into the
/// evaluator's native representation once. Cloning this handle only increments
/// an [`Arc`] reference count.
#[derive(Clone)]
pub struct PreparedValue {
    value: Arc<dyn Val>,
}

impl PreparedValue {
    /// Convert a public CEL [`Value`] into a reusable native value.
    pub fn try_from_value(value: Value) -> Result<Self, ExecutionError> {
        let value: Box<dyn Val> = value.try_into()?;
        Ok(Self::from_boxed(value))
    }

    pub(crate) fn from_boxed(value: Box<dyn Val>) -> Self {
        Self {
            value: Arc::from(value),
        }
    }

    fn as_val(&self) -> &dyn Val {
        self.value.as_ref()
    }

    /// Return the CEL runtime type name without formatting the contained value.
    pub fn type_name(&self) -> &str {
        self.value.get_type().name()
    }
}

impl fmt::Debug for PreparedValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PreparedValue")
            .field("type", &self.type_name())
            .finish_non_exhaustive()
    }
}

impl TryFrom<Value> for PreparedValue {
    type Error = ExecutionError;

    fn try_from(value: Value) -> Result<Self, Self::Error> {
        Self::try_from_value(value)
    }
}

/// Context is a collection of variables and functions that can be used
/// by the interpreter to resolve expressions.
///
/// The context can be either a parent context, or a child context. A
/// parent context is created by default and contains all of the built-in
/// functions. A child context can be created by calling `.new_inner_scope()`. The
/// child context has it's own variables (which can be added to), but it
/// will also reference the parent context. This allows for variables to
/// be overridden within the child context while still being able to
/// resolve variables in the child's parents. You can have theoretically
/// have an infinite number of child contexts that reference each-other.
///
/// So why is this important? Well some CEL-macros such as the `.map` macro
/// declare intermediate user-specified identifiers that should only be
/// available within the macro, and should not override variables in the
/// parent context. The `.map` macro can create a child context from the parent, add the
/// intermediate identifier to the child context, and then evaluate the
/// map expression.
///
/// Intermediate variable stored in child context
///               ↓
/// [1, 2, 3].map(x, x * 2) == [2, 4, 6]
///                  ↑
/// Only in scope for the duration of the map expression
///
pub enum Context<'a> {
    Root {
        functions: FunctionRegistry,
        variables: BTreeMap<String, PreparedValue>,
        resolver: Option<&'a dyn VariableResolver>,
        env: Arc<Env>,
    },
    Child {
        parent: &'a Context<'a>,
        variables: BTreeMap<String, PreparedValue>,
        resolver: Option<&'a dyn VariableResolver>,
    },
}

impl<'a> Context<'a> {
    pub fn add_variable<S, V>(
        &mut self,
        name: S,
        value: V,
    ) -> Result<(), <V as TryIntoValue>::Error>
    where
        S: Into<String>,
        V: TryIntoValue,
    {
        let value = value.try_into_value()?;
        self.add_variable_from_value(name, value);
        Ok(())
    }

    pub fn add_variable_from_value<S, V>(&mut self, name: S, value: V)
    where
        S: Into<String>,
        V: Into<Value>,
    {
        let value: Box<dyn Val> = value.into().try_into().unwrap();
        self.add_prepared_variable(name, PreparedValue::from_boxed(value));
    }

    /// Add or replace a variable using a prepared shared value.
    ///
    /// Inserting a clone of a retained [`PreparedValue`] is independent of the
    /// size of the value tree.
    pub fn add_prepared_variable<S>(&mut self, name: S, value: PreparedValue)
    where
        S: Into<String>,
    {
        match self {
            Context::Root { variables, .. } | Context::Child { variables, .. } => {
                variables.insert(name.into(), value);
            }
        }
    }

    pub(crate) fn add_variable_as_val<S>(&mut self, name: S, value: Box<dyn Val>)
    where
        S: Into<String>,
    {
        self.add_prepared_variable(name, PreparedValue::from_boxed(value));
    }

    pub fn set_variable_resolver(&mut self, r: &'a dyn VariableResolver) {
        match self {
            Context::Root { resolver, .. } => {
                *resolver = Some(r);
            }
            Context::Child { resolver, .. } => {
                *resolver = Some(r);
            }
        }
    }

    pub fn get_variable<S>(&'a self, name: S) -> Option<Cow<'a, dyn Val>>
    where
        S: AsRef<str>,
    {
        let name = name.as_ref();
        match self {
            Context::Child {
                variables,
                parent,
                resolver,
            } => resolver
                .and_then(|r| {
                    r.resolve(name)
                        .map(|v| Cow::<dyn Val>::Owned(v.try_into().unwrap()))
                })
                .or_else(|| {
                    variables
                        .get(name)
                        .map(|value| Cow::<dyn Val>::Borrowed(value.as_val()))
                        .or_else(|| parent.get_variable(name))
                }),
            Context::Root {
                variables,
                resolver,
                ..
            } => resolver
                .and_then(|r| {
                    r.resolve(name)
                        .map(|v| Cow::<dyn Val>::Owned(v.try_into().unwrap()))
                })
                .or_else(|| {
                    variables
                        .get(name)
                        .map(|value| Cow::<dyn Val>::Borrowed(value.as_val()))
                }),
        }
    }

    pub(crate) fn env(&self) -> &Env {
        match self {
            Context::Root { env, .. } => env.as_ref(),
            Context::Child { parent, .. } => parent.env(),
        }
    }

    #[allow(dead_code)]
    pub(crate) fn get_function(&self, name: &str) -> Option<&Function> {
        match self {
            Context::Root { functions, .. } => functions.get(name),
            Context::Child { parent, .. } => parent.get_function(name),
        }
    }

    pub fn add_function<T: 'static, F>(&mut self, name: &str, value: F)
    where
        F: IntoFunction<T> + 'static + Send + Sync,
    {
        if let Context::Root { functions, .. } = self {
            functions.add(name, value);
        };
    }

    pub fn resolve(&self, expr: &Expression) -> Result<Value, ExecutionError> {
        Value::resolve(expr, self)
    }

    pub fn resolve_all(&self, exprs: &[Expression]) -> Result<Value, ExecutionError> {
        Value::resolve_all(exprs, self)
    }

    pub fn new_inner_scope(&self) -> Context<'_> {
        Context::Child {
            parent: self,
            variables: Default::default(),
            resolver: None,
        }
    }

    /// Constructs a new empty context with no variables or functions.
    ///
    /// If you're looking for a context that has all the standard methods, functions
    /// and macros already added to the context, use [`Context::default`] instead.
    ///
    /// # Example
    /// ```
    /// use cel::Context;
    /// let mut context = Context::empty();
    /// context.add_function("add", |a: i64, b: i64| a + b);
    /// ```
    pub fn empty() -> Self {
        Context::Root {
            env: Arc::new(Env::default()),
            variables: Default::default(),
            functions: Default::default(),
            resolver: None,
        }
    }

    pub fn with_env(env: Arc<Env>) -> Self {
        Context::Root {
            env,
            variables: Default::default(),
            functions: Default::default(),
            resolver: None,
        }
    }
}

impl Default for Context<'_> {
    fn default() -> Self {
        Context::Root {
            env: Arc::new(Env::stdlib()),
            variables: Default::default(),
            functions: Default::default(),
            resolver: None,
        }
    }
}

/// VariableResolver implements a custom resolver for variables that is consulted before looking at
/// variables added to the context. This allows dynamic variables, or avoiding HashMap lookup/creation.
///
///
/// # Example
/// ```
/// struct ValueContext {
///     request: cel::Value,
///     response: cel::Value,
/// }
///
/// impl cel::context::VariableResolver for ValueContext {
///     fn resolve(&self, variable: &str) -> Option<cel::Value> {
///         match variable {
///             "request" => Some(self.request.clone()),
///             "response" => Some(self.response.clone()),
///             _ => None,
///         }
///     }
/// }
/// ```
pub trait VariableResolver: Send + Sync {
    fn resolve(&self, variable: &str) -> Option<Value>;
}

impl<T: VariableResolver> VariableResolver for Box<T> {
    fn resolve(&self, variable: &str) -> Option<Value> {
        (**self).resolve(variable)
    }
}

impl<T: VariableResolver> VariableResolver for Arc<T> {
    fn resolve(&self, variable: &str) -> Option<Value> {
        (**self).resolve(variable)
    }
}

impl<T: VariableResolver> VariableResolver for &T {
    fn resolve(&self, variable: &str) -> Option<Value> {
        (**self).resolve(variable)
    }
}

#[cfg(test)]
mod test {
    use super::{Context, PreparedValue};
    use crate::{Program, Value};
    use std::collections::HashMap;

    fn assert_send_sync<T: Send + Sync>() {}

    #[test]
    fn context_and_prepared_value_are_send_and_sync() {
        assert_send_sync::<Context>();
        assert_send_sync::<PreparedValue>();
    }

    #[test]
    fn prepared_variable_can_be_reused_and_replaced() {
        let first =
            PreparedValue::try_from_value(Value::from(HashMap::from([("value", 1)]))).unwrap();
        let second =
            PreparedValue::try_from_value(Value::from(HashMap::from([("value", 2)]))).unwrap();
        let program = Program::compile("data.value").unwrap();
        let mut context = Context::default();

        context.add_prepared_variable("data", first.clone());
        assert_eq!(program.execute(&context), Ok(Value::Int(1)));

        context.add_prepared_variable("data", second);
        assert_eq!(program.execute(&context), Ok(Value::Int(2)));

        context.add_prepared_variable("data", first);
        assert_eq!(program.execute(&context), Ok(Value::Int(1)));
    }
}
