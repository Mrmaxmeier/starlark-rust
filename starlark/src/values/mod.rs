// Copyright 2018 The Starlark in Rust Authors
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     https://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! The values module define a trait `TypedValue` that defines the attribute of
//! any value in Starlark and a few macro to help implementing this trait.
//! The `Value` struct defines the actual structure holding a TypedValue. It is mostly used to
//! enable mutable and Rc behavior over a TypedValue.
//! This modules also defines this traits for the basic immutable values: int, bool and NoneType.
//! Sub-modules implement other common types of all Starlark dialect.
//!
//! __Note__: we use _sequence_, _iterable_ and _indexable_ according to the
//! definition in the [Starlark specification](
//! https://github.com/google/skylark/blob/a0e5de7e63b47e716cca7226662a4c95d47bf873/doc/spec.md#sequence-types).
//! We also use the term _container_ for denoting any of those type that can hold several values.
//!
//!
//! # Defining a new type
//!
//! Defining a new Starlark type can be done by implenting the [TypedValue](trait.TypedValue.html)
//! trait. All method of that trait are operation needed by Starlark interpreter to understand the
//! type. The [not_supported!](macro.not_supported.html) macro let us tell which operation is not
//! supported by the current type.
//!
//! For example the `NoneType` trait implementation is the following:
//!
//! ```rust,ignore
//! /// Define the NoneType type
//! impl TypedValue for Option<()> {
//!     immutable!();
//!     any!();  // Generally you don't want to implement as_any() and as_any_mut() yourself.
//!     fn to_str(&self) -> String {
//!         "None".to_owned()
//!     }
//!     fn to_repr(&self) -> String {
//!         self.to_str()
//!     }
//!     not_supported!(to_int);
//!     fn get_type(&self) -> &'static str {
//!         "NoneType"
//!     }
//!     fn to_bool(&self) -> bool {
//!         false
//!     }
//!     // just took the result of hash(None) in macos python 2.7.10 interpreter.
//!     fn get_hash(&self) -> Result<u64, ValueError> {
//!         Ok(9223380832852120682)
//!     }
//!     fn compare(&self, other: &Value) -> Ordering { default_compare(self, other) }
//!     not_supported!(binop);
//!     not_supported!(container);
//!     not_supported!(function);
//! }
//! ```
//!
//! In addition to the `TypedValue` trait, it is recommended to implement the `From` trait
//! for all type that can convert to the added type but parameterized it with the `Into<Value>`
//! type. For example the unary tuple `From` trait is defined as followed:
//!
//! ```rust,ignore
//! impl<T: Into<Value>> From<(T,)> for Tuple {
//!     fn from(a: (T,)) -> Tuple {
//!         Tuple { content: vec![a.0.into()] }
//!     }
//! }
//! ```
use crate::environment::Environment;
use crate::values::error::ValueError;
use codemap_diagnostic::Level;
use linked_hash_map::LinkedHashMap;
use std::any::Any;
use std::cell::RefCell;
use std::cmp::Ordering;
use std::fmt;
use std::ops::Deref;
use std::rc::Rc;

// Maximum recursion level for comparison
// TODO(dmarting): those are rather short, maybe make it configurable?
#[cfg(debug_assertions)]
const MAX_RECURSION: u32 = 200;

#[cfg(not(debug_assertions))]
const MAX_RECURSION: u32 = 3000;

/// A value in Starlark.
///
/// This is a wrapper around a [TypedValue] which is cheap to clone and safe to pass around.
#[derive(Clone)]
pub struct Value(Rc<RefCell<dyn TypedValue>>);

pub type ValueResult = Result<Value, ValueError>;

impl Value {
    /// Create a new `Value` from a static value.
    pub fn new<T: 'static + TypedValue>(t: T) -> Value {
        Value(Rc::new(RefCell::new(t)))
    }

    /// Clone for inserting into the other container, using weak reference if we do a
    /// recursive insertion.
    pub fn clone_for_container(&self, other: &dyn TypedValue) -> Result<Value, ValueError> {
        if self.is_descendant(other) {
            Err(ValueError::UnsupportedRecursiveDataStructure)
        } else {
            Ok(self.clone())
        }
    }

    pub fn clone_for_container_value(&self, other: &Value) -> Result<Value, ValueError> {
        if self.is_descendant(other.0.borrow().deref()) {
            Err(ValueError::UnsupportedRecursiveDataStructure)
        } else {
            Ok(self.clone())
        }
    }

    /// Determine if the value pointed by other is a descendant of self
    pub fn is_descendant_value(&self, other: &Value) -> bool {
        let borrowed = other.0.borrow();
        self.is_descendant(borrowed.deref())
    }

    /// Return true if other is pointing to the same value as self
    pub fn same_as(&self, other: &dyn TypedValue) -> bool {
        // We use raw pointers..
        let p: *const dyn TypedValue = other;
        let p1: *const dyn TypedValue = self.0.as_ptr();
        p1 == p
    }
}

/// A trait for a value with a type that all variable container
/// will implement.
pub trait TypedValue {
    /// Return true if the value is immutable.
    fn immutable(&self) -> bool;

    /// Convert to an Any. This allows for operation on the native type.
    /// You most certainly don't want to implement it yourself but rather use the `any!` macro.
    fn as_any(&self) -> &dyn Any;

    /// Convert to a mutable Any. This allows for operation on the native type.
    /// You most certainly don't want to implement it yourself but rather use the `any!` macro.
    fn as_any_mut(&mut self) -> &mut dyn Any;

    /// Freeze, i.e. make the value immutable.
    fn freeze(&mut self);

    /// Freeze for interation, i.e. make the value temporary immutable. This does not
    /// propage to child element commpared to the freeze() function.
    fn freeze_for_iteration(&mut self);

    /// Unfreeze after a call to freeze_for_iteration(), i.e. make the value mutable
    /// again.
    fn unfreeze_for_iteration(&mut self);

    /// Return a string describing of self, as returned by the str() function.
    fn to_str(&self) -> String {
        self.to_repr()
    }

    /// Return a string representation of self, as returned by the repr() function.
    fn to_repr(&self) -> String {
        format!("<{}>", self.get_type())
    }

    /// Return a string describing the type of self, as returned by the type() function.
    fn get_type(&self) -> &'static str;

    /// Convert self to a Boolean truth value, as returned by the bool() function.
    fn to_bool(&self) -> bool {
        // Return `true` by default, because this is default when implementing
        // custom types in Python: https://docs.python.org/release/2.5.2/lib/truth.html
        true
    }

    /// Convert self to a integer value, as returned by the int() function if the type is numeric
    /// (not for string).
    fn to_int(&self) -> Result<i64, ValueError> {
        Err(ValueError::OperationNotSupported {
            op: "int()".to_owned(),
            left: self.get_type().to_owned(),
            right: None,
        })
    }

    /// Return a hash code for self, as returned by the hash() function, or
    /// OperationNotSupported if there is no hash for this value (e.g. list).
    fn get_hash(&self) -> Result<u64, ValueError> {
        Err(ValueError::NotHashableValue)
    }

    /// Returns true if `other` is a descendent of the current value, used for sanity checks.
    fn is_descendant(&self, other: &dyn TypedValue) -> bool;

    /// Compare `self` with `other`.
    ///
    /// This method returns a result of type
    /// [Ordering](https://doc.rust-lang.org/std/cmp/enum.Ordering.html). If it cannot perform
    /// the comparison it should return `self.get_type().cmp(other.get_type())`.
    ///
    /// This assumption work since we consider that `a < b <=> b > a`.
    ///
    /// __Note__: This does not use the
    ///       (PartialOrd)[https://doc.rust-lang.org/std/cmp/trait.PartialOrd.html] trait as
    ///       the trait needs to know the actual type of the value we compare.
    ///
    /// The extraneous recursion parameter is used to detect deep recursion.
    fn compare(&self, other: &dyn TypedValue, recursion: u32) -> Result<Ordering, ValueError>;

    /// Perform a call on the object, only meaningfull for function object.
    ///
    /// For instance, if this object is a callable (i.e. a function or a method) that adds 2
    /// integers then `self.call(vec![Value::new(1), Value::new(2)], HashMap::new(),
    /// None, None)` would return `Ok(Value::new(3))`.
    ///
    /// # Parameters
    ///
    /// * call_stack: the calling stack, to detect recursion
    /// * env: the environment for the call.
    /// * positional: the list of arguments passed positionally.
    /// * named: the list of argument that were named.
    /// * args: if provided, the `*args` argument.
    /// * kwargs: if provided, the `**kwargs` argument.
    fn call(
        &self,
        _call_stack: &[(String, String)],
        _env: Environment,
        _positional: Vec<Value>,
        _named: LinkedHashMap<String, Value>,
        _args: Option<Value>,
        _kwargs: Option<Value>,
    ) -> ValueResult {
        Err(ValueError::OperationNotSupported {
            op: "call()".to_owned(),
            left: self.get_type().to_owned(),
            right: None,
        })
    }

    /// Perform an array or dictionary indirection.
    ///
    /// This returns the result of `a[index]` if `a` is indexable.
    fn at(&self, index: Value) -> ValueResult {
        Err(ValueError::OperationNotSupported {
            op: "[]".to_owned(),
            left: self.get_type().to_owned(),
            right: Some(index.get_type().to_owned()),
        })
    }

    /// Set the value at `index` with `new_value`.
    ///
    /// This method should error with `ValueError::CannotMutateImmutableValue` if the value was
    /// frozen (but with `ValueError::OperationNotSupported` if the operation is not supported
    /// on this value, even if the value is immutable, e.g. for numbers).
    fn set_at(&mut self, index: Value, _new_value: Value) -> Result<(), ValueError> {
        Err(ValueError::OperationNotSupported {
            op: "[] =".to_owned(),
            left: self.get_type().to_owned(),
            right: Some(index.get_type().to_owned()),
        })
    }

    /// Extract a slice of the underlying object if the object is indexable. The result will be
    /// object between `start` and `stop` (both of them are added length() if negative and then
    /// clamped between 0 and length()). `stride` indicates the direction.
    ///
    /// # Parameters
    ///
    /// * start: the start of the slice.
    /// * stop: the end of the slice.
    /// * stride: the direction of slice,
    ///
    /// # Examples
    ///
    /// ```rust
    /// # use starlark::values::*;
    /// # use starlark::values::string;
    /// # assert!(
    /// // Remove the first element: "abc"[1:] == "bc".
    /// Value::from("abc").slice(Some(Value::from(1)), None, None).unwrap() == Value::from("bc")
    /// # );
    /// # assert!(
    /// // Remove the last element: "abc"[:-1] == "ab".
    /// Value::from("abc").slice(None, Some(Value::from(-1)), None).unwrap()
    ///    == Value::from("ab")
    /// # );
    /// # assert!(
    /// // Remove the first and the last element: "abc"[1:-1] == "b".
    /// Value::from("abc").slice(Some(Value::from(1)), Some(Value::from(-1)), None).unwrap()
    ///    == Value::from("b")
    /// # );
    /// # assert!(
    /// // Select one element out of 2, skipping the first: "banana"[1::2] == "aaa".
    /// Value::from("banana").slice(Some(Value::from(1)), None, Some(Value::from(2))).unwrap()
    ///    == Value::from("aaa")
    /// # );
    /// # assert!(
    /// // Select one element out of 2 in reverse order, starting at index 4:
    /// //   "banana"[4::-2] = "nnb"
    /// Value::from("banana").slice(Some(Value::from(4)), None, Some(Value::from(-2))).unwrap()
    ///    == Value::from("nnb")
    /// # );
    /// ```
    fn slice(
        &self,
        _start: Option<Value>,
        _stop: Option<Value>,
        _stride: Option<Value>,
    ) -> ValueResult {
        Err(ValueError::OperationNotSupported {
            op: "[::]".to_owned(),
            left: self.get_type().to_owned(),
            right: None,
        })
    }

    /// Returns an iterator over the value of this container if this value hold an iterable
    /// container.
    fn iter<'a>(&'a self) -> Result<Box<dyn Iterator<Item = Value> + 'a>, ValueError> {
        Err(ValueError::TypeNotX {
            object_type: self.get_type().to_owned(),
            op: "iterable".to_owned(),
        })
    }

    /// Returns the length of the value, if this value is a sequence.
    fn length(&self) -> Result<i64, ValueError> {
        Err(ValueError::OperationNotSupported {
            op: "len()".to_owned(),
            left: self.get_type().to_owned(),
            right: None,
        })
    }

    /// Get an attribute for the current value as would be returned by dotted expression (i.e.
    /// `a.attribute`).
    ///
    /// __Note__: this does not handle native methods which are handled through universe.
    fn get_attr(&self, attribute: &str) -> ValueResult {
        Err(ValueError::OperationNotSupported {
            op: format!(".{}", attribute),
            left: self.get_type().to_owned(),
            right: None,
        })
    }

    /// Return true if an attribute of name `attribute` exists for the current value.
    ///
    /// __Note__: this does not handle native methods which are handled through universe.
    fn has_attr(&self, _attribute: &str) -> Result<bool, ValueError> {
        Err(ValueError::OperationNotSupported {
            op: "has_attr()".to_owned(),
            left: self.get_type().to_owned(),
            right: None,
        })
    }

    /// Set the attribute named `attribute` of the current value to `new_value` (e.g.
    /// `a.attribute = new_value`).
    ///
    /// This method should error with `ValueError::CannotMutateImmutableValue` if the value was
    /// frozen or the attribute is immutable (but with `ValueError::OperationNotSupported`
    /// if the operation is not supported on this value, even if the self is immutable,
    /// e.g. for numbers).
    fn set_attr(&mut self, attribute: &str, _new_value: Value) -> Result<(), ValueError> {
        Err(ValueError::OperationNotSupported {
            op: format!(".{} =", attribute),
            left: self.get_type().to_owned(),
            right: None,
        })
    }

    /// Return a vector of string listing all attribute of the current value, excluding native
    /// methods.
    fn dir_attr(&self) -> Result<Vec<String>, ValueError> {
        Err(ValueError::OperationNotSupported {
            op: "dir()".to_owned(),
            left: self.get_type().to_owned(),
            right: None,
        })
    }

    /// Tell wether `other` is in the current value, if it is a container.
    ///
    /// Non container value should return an error `ValueError::OperationNotSupported`.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # use starlark::values::*;
    /// # use starlark::values::string;
    /// // "a" in "abc" == True
    /// assert!(Value::from("abc").is_in(&Value::from("a")).unwrap().to_bool());
    /// // "b" in "abc" == True
    /// assert!(Value::from("abc").is_in(&Value::from("b")).unwrap().to_bool());
    /// // "z" in "abc" == False
    /// assert!(!Value::from("abc").is_in(&Value::from("z")).unwrap().to_bool());
    /// ```
    fn is_in(&self, other: &Value) -> Result<bool, ValueError> {
        Err(ValueError::OperationNotSupported {
            op: "in".to_owned(),
            left: other.get_type().to_owned(),
            right: Some(self.get_type().to_owned()),
        })
    }

    /// Apply the `+` unary operator to the current value.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[macro_use] extern crate starlark;
    /// # use starlark::values::*;
    /// # fn main() {
    /// assert_eq!(1, int_op!(1.plus()));  // 1.plus() = +1 = 1
    /// # }
    /// ```
    fn plus(&self) -> ValueResult {
        Err(ValueError::OperationNotSupported {
            op: "+".to_owned(),
            left: self.get_type().to_owned(),
            right: None,
        })
    }

    /// Apply the `-` unary operator to the current value.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[macro_use] extern crate starlark;
    /// # use starlark::values::*;
    /// # fn main() {
    /// assert_eq!(-1, int_op!(1.minus()));  // 1.minus() = -1
    /// # }
    /// ```
    fn minus(&self) -> ValueResult {
        Err(ValueError::OperationNotSupported {
            op: "-".to_owned(),
            left: self.get_type().to_owned(),
            right: None,
        })
    }

    /// Add `other` to the current value.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[macro_use] extern crate starlark;
    /// # use starlark::values::*;
    /// # fn main() {
    /// assert_eq!(3, int_op!(1.add(2)));  // 1.add(2) = 1 + 2 = 3
    /// # }
    /// ```
    fn add(&self, other: Value) -> ValueResult {
        Err(ValueError::OperationNotSupported {
            op: "+".to_owned(),
            left: self.get_type().to_owned(),
            right: Some(other.get_type().to_owned()),
        })
    }

    /// Substract `other` from the current value.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[macro_use] extern crate starlark;
    /// # use starlark::values::*;
    /// # fn main() {
    /// assert_eq!(-1, int_op!(1.sub(2)));  // 1.sub(2) = 1 - 2 = -1
    /// # }
    /// ```
    fn sub(&self, other: Value) -> ValueResult {
        Err(ValueError::OperationNotSupported {
            op: "-".to_owned(),
            left: self.get_type().to_owned(),
            right: Some(other.get_type().to_owned()),
        })
    }

    /// Multiply the current value with `other`.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[macro_use] extern crate starlark;
    /// # use starlark::values::*;
    /// # fn main() {
    /// assert_eq!(6, int_op!(2.mul(3)));  // 2.mul(3) = 2 * 3 = 6
    /// # }
    /// ```
    fn mul(&self, other: Value) -> ValueResult {
        Err(ValueError::OperationNotSupported {
            op: "*".to_owned(),
            left: self.get_type().to_owned(),
            right: Some(other.get_type().to_owned()),
        })
    }

    /// Apply the percent operator between the current value and `other`.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[macro_use] extern crate starlark;
    /// # use starlark::values::*;
    /// # use starlark::values::string;
    /// # fn main() {
    /// // Remainder of the floored division: 5.percent(3) = 5 % 3 = 2
    /// assert_eq!(2, int_op!(5.percent(3)));
    /// // String formatting: "a {} c" % 3 == "a 3 c"
    /// assert_eq!(Value::from("a 3 c"), Value::from("a %s c").percent(Value::from(3)).unwrap());
    /// # }
    /// ```
    fn percent(&self, other: Value) -> ValueResult {
        Err(ValueError::OperationNotSupported {
            op: "%".to_owned(),
            left: self.get_type().to_owned(),
            right: Some(other.get_type().to_owned()),
        })
    }

    /// Divide the current value with `other`.  division.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[macro_use] extern crate starlark;
    /// # use starlark::values::*;
    /// # fn main() {
    /// assert_eq!(3, int_op!(7.div(2)));  // 7.div(2) = 7 / 2 = 3
    /// # }
    /// ```
    fn div(&self, other: Value) -> ValueResult {
        Err(ValueError::OperationNotSupported {
            op: "/".to_owned(),
            left: self.get_type().to_owned(),
            right: Some(other.get_type().to_owned()),
        })
    }

    /// Floor division between the current value and `other`.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[macro_use] extern crate starlark;
    /// # use starlark::values::*;
    /// # fn main() {
    /// assert_eq!(3, int_op!(7.floor_div(2)));  // 7.div(2) = 7 / 2 = 3
    /// # }
    /// ```
    fn floor_div(&self, other: Value) -> ValueResult {
        Err(ValueError::OperationNotSupported {
            op: "//".to_owned(),
            left: self.get_type().to_owned(),
            right: Some(other.get_type().to_owned()),
        })
    }

    /// Apply the operator pipe to the current value and `other`.
    ///
    /// This is usually the union on set.
    fn pipe(&self, other: Value) -> ValueResult {
        Err(ValueError::OperationNotSupported {
            op: "|".to_owned(),
            left: self.get_type().to_owned(),
            right: Some(other.get_type().to_owned()),
        })
    }
}

impl fmt::Debug for dyn TypedValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        write!(f, "Value[{}]({})", self.get_type(), self.to_repr())
    }
}

#[macro_export]
macro_rules! any {
    () => {
        fn as_any(&self) -> &dyn ::std::any::Any {
            self
        }

        fn as_any_mut(&mut self) -> &mut dyn ::std::any::Any {
            self
        }
    }
}

/// A default implementation of the compare function, this can be used if the two types of
/// value are differents or numeric. Custom types should implement their own comparison for the
/// last case.
pub fn default_compare(v1: &dyn TypedValue, v2: &dyn TypedValue) -> Result<Ordering, ValueError> {
    Ok(match (v1.get_type(), v2.get_type()) {
        ("bool", "bool") | ("bool", "int") | ("int", "bool") | ("int", "int") => {
            v1.to_int()?.cmp(&(v2.to_int()?))
        }
        ("bool", ..) | ("int", ..) => Ordering::Less,
        (.., "bool") | (.., "int") => Ordering::Greater,
        (x, y) => x.cmp(y),
    })
}

macro_rules! default_compare {
    () => {
        fn compare(&self, other: &dyn TypedValue, _recursion: u32) -> Result<Ordering, ValueError> { default_compare(self, other) }
    }
}

/// Declare the value as immutable.
#[macro_export]
macro_rules! immutable {
    () => {
        fn immutable(&self) -> bool { true }
        fn freeze(&mut self) {}
        fn freeze_for_iteration(&mut self) {}
        fn unfreeze_for_iteration(&mut self) {}
    }
}

/// A helper enum for defining the level of mutability of an iterable.
///
/// # Examples
///
/// It is made to be used together with default_iterable_mutability! macro, e.g.:
///
/// ```rust,ignore
/// pub struct MyIterable {
///   IterableMutability mutability;
///   // ...
/// }
///
/// impl Value for MyIterable {
///    // define freeze* functions
///    define_iterable_mutability!(mutability)
///    
///    // Later you can use mutability.test()? to
///    // return an error if trying to mutate a frozen object.
/// }
/// ```
#[derive(PartialEq, Eq, Hash, Debug)]
pub enum IterableMutability {
    Mutable,
    Immutable,
    FrozenForIteration,
}

impl IterableMutability {
    /// Tests the mutability value and return the appropriate error
    ///
    /// This method is to be called simply `mutability.test()?` to return
    /// an error if the current container is no longer mutable.
    pub fn test(&self) -> Result<(), ValueError> {
        match self {
            IterableMutability::Mutable => Ok(()),
            IterableMutability::Immutable => Err(ValueError::CannotMutateImmutableValue),
            IterableMutability::FrozenForIteration => Err(ValueError::MutationDuringIteration),
        }
    }

    /// Freezes the current value, can be used when implementing the `freeze` function
    /// of the [TypedValue] trait.
    pub fn freeze(&mut self) {
        *self = IterableMutability::Immutable;
    }

    /// Tells wether the current value define a permanently immutable function, to be used
    /// to implement the `immutable` function of the [TypedValue] trait.
    pub fn immutable(&self) -> bool {
        *self == IterableMutability::Immutable
    }

    /// Freezes the current value for iterating over, to be used to implement the
    /// `freeze_for_iteration` function of the [TypedValue] trait.
    pub fn freeze_for_iteration(&mut self) {
        if self.immutable() {
            return;
        }
        *self = IterableMutability::FrozenForIteration
    }

    /// Unfreezes the current value for iterating over, to be used to implement the
    /// `unfreeze_for_iteration` function of the [TypedValue] trait.
    pub fn unfreeze_for_iteration(&mut self) {
        if self.immutable() {
            return;
        }
        *self = IterableMutability::Mutable
    }
}

/// Define functions *freeze_for_iteration/immutable for type using [IterableMutability].
///
/// E.g., if `mutability` is a field of type [IterableMutability], then
/// `define_iterable_mutability(mutability)` would define the four function
/// `immutable`, `freeze_for_iteration` and `unfreeze_for_iteration`
/// for the current trait implementation.
#[macro_export]
macro_rules! define_iterable_mutability {
    ($name: ident) => {
        fn immutable(&self) -> bool { self.$name.immutable() }
        fn freeze_for_iteration(&mut self) { self.$name.freeze_for_iteration() }
        fn unfreeze_for_iteration(&mut self) { self.$name.unfreeze_for_iteration() }
    }
}

impl Value {
    pub fn any_apply<F, Return>(&self, f: F) -> Return
    where
        F: FnOnce(&dyn Any) -> Return,
    {
        let borrowed = self.0.borrow();
        f(borrowed.as_any())
    }

    pub fn any_apply_mut<F, Return>(&mut self, f: F) -> Return
    where
        F: FnOnce(&mut dyn Any) -> Return,
    {
        let mut borrowed = self.0.borrow_mut();
        f(borrowed.as_any_mut())
    }

    pub fn immutable(&self) -> bool {
        let borrowed = self.0.borrow();
        borrowed.immutable()
    }
    pub fn freeze(&mut self) {
        let mut borrowed = self.0.borrow_mut();
        borrowed.freeze()
    }
    pub fn freeze_for_iteration(&mut self) {
        let mut borrowed = self.0.borrow_mut();
        borrowed.freeze_for_iteration()
    }
    pub fn unfreeze_for_iteration(&mut self) {
        let mut borrowed = self.0.borrow_mut();
        borrowed.unfreeze_for_iteration()
    }
    pub fn to_str(&self) -> String {
        self.0.borrow().to_str()
    }
    pub fn to_repr(&self) -> String {
        self.0.borrow().to_repr()
    }
    pub fn get_type(&self) -> &'static str {
        let borrowed = self.0.borrow();
        borrowed.get_type()
    }
    pub fn to_bool(&self) -> bool {
        let borrowed = self.0.borrow();
        borrowed.to_bool()
    }
    pub fn to_int(&self) -> Result<i64, ValueError> {
        let borrowed = self.0.borrow();
        borrowed.to_int()
    }
    pub fn get_hash(&self) -> Result<u64, ValueError> {
        let borrowed = self.0.borrow();
        borrowed.get_hash()
    }
    pub fn compare(&self, other: &Value, recursion: u32) -> Result<Ordering, ValueError> {
        self.compare_underlying(other.0.borrow().deref(), recursion)
    }

    pub fn compare_underlying(
        &self,
        other: &dyn TypedValue,
        recursion: u32,
    ) -> Result<Ordering, ValueError> {
        let borrowed = self.0.borrow();
        if recursion > MAX_RECURSION {
            return Err(ValueError::TooManyRecursionLevel);
        }
        if ::std::ptr::eq(borrowed.deref(), other) {
            // Special case for recursive structure, stop if we are pointing to the same object.
            Ok(Ordering::Equal)
        } else {
            borrowed.compare(other, recursion)
        }
    }

    pub fn is_descendant(&self, other: &dyn TypedValue) -> bool {
        if self.same_as(other) {
            return true;
        }
        let try_borrowed = self.0.try_borrow();
        if let Ok(borrowed) = try_borrowed {
            borrowed.is_descendant(other)
        } else {
            // We have already borrowed mutably this value, which means we are trying to mutate it, assigning other to it.
            true
        }
    }

    pub fn call(
        &self,
        call_stack: &[(String, String)],
        env: Environment,
        positional: Vec<Value>,
        named: LinkedHashMap<String, Value>,
        args: Option<Value>,
        kwargs: Option<Value>,
    ) -> ValueResult {
        let borrowed = self.0.borrow();
        borrowed.call(call_stack, env, positional, named, args, kwargs)
    }
    pub fn at(&self, index: Value) -> ValueResult {
        let borrowed = self.0.borrow();
        borrowed.at(index)
    }
    pub fn set_at(&mut self, index: Value, new_value: Value) -> Result<(), ValueError> {
        let mut borrowed = self.0.borrow_mut();
        borrowed.set_at(index, new_value)
    }
    pub fn slice(
        &self,
        start: Option<Value>,
        stop: Option<Value>,
        stride: Option<Value>,
    ) -> ValueResult {
        let borrowed = self.0.borrow_mut();
        borrowed.slice(start, stop, stride)
    }
    pub fn iter<'a>(&'a self) -> Result<Box<dyn Iterator<Item = Value> + 'a>, ValueError> {
        let borrowed = self.0.borrow();
        let v: Vec<Value> = borrowed.iter()?.collect();
        Ok(Box::new(v.into_iter()))
    }
    pub fn length(&self) -> Result<i64, ValueError> {
        let borrowed = self.0.borrow();
        borrowed.length()
    }
    pub fn get_attr(&self, attribute: &str) -> ValueResult {
        let borrowed = self.0.borrow();
        borrowed.get_attr(attribute)
    }
    pub fn has_attr(&self, attribute: &str) -> Result<bool, ValueError> {
        let borrowed = self.0.borrow();
        borrowed.has_attr(attribute)
    }
    pub fn set_attr(&mut self, attribute: &str, new_value: Value) -> Result<(), ValueError> {
        let mut borrowed = self.0.borrow_mut();
        borrowed.set_attr(attribute, new_value)
    }
    pub fn dir_attr(&self) -> Result<Vec<String>, ValueError> {
        let borrowed = self.0.borrow();
        borrowed.dir_attr()
    }
    pub fn is_in(&self, other: &Value) -> Result<bool, ValueError> {
        let borrowed = self.0.borrow();
        borrowed.is_in(other)
    }
    pub fn plus(&self) -> ValueResult {
        let borrowed = self.0.borrow();
        borrowed.plus()
    }
    pub fn minus(&self) -> ValueResult {
        let borrowed = self.0.borrow();
        borrowed.minus()
    }
    pub fn add(&self, other: Value) -> ValueResult {
        let borrowed = self.0.borrow();
        borrowed.add(other)
    }
    pub fn sub(&self, other: Value) -> ValueResult {
        let borrowed = self.0.borrow();
        borrowed.sub(other)
    }
    pub fn mul(&self, other: Value) -> ValueResult {
        let borrowed = self.0.borrow();
        borrowed.mul(other)
    }
    pub fn percent(&self, other: Value) -> ValueResult {
        let borrowed = self.0.borrow();
        borrowed.percent(other)
    }
    pub fn div(&self, other: Value) -> ValueResult {
        let borrowed = self.0.borrow();
        borrowed.div(other)
    }
    pub fn floor_div(&self, other: Value) -> ValueResult {
        let borrowed = self.0.borrow();
        borrowed.floor_div(other)
    }
    pub fn pipe(&self, other: Value) -> ValueResult {
        let borrowed = self.0.borrow();
        borrowed.pipe(other)
    }
}

impl fmt::Debug for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        let borrowed = self.0.borrow();
        write!(f, "{:?}", borrowed)
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        write!(f, "{}", self.to_str())
    }
}

impl PartialEq for Value {
    fn eq(&self, other: &Value) -> bool {
        self.compare(other, 0) == Ok(Ordering::Equal)
    }
}
impl Eq for Value {}

impl Ord for Value {
    fn cmp(&self, other: &Value) -> Ordering {
        self.compare(other, 0).unwrap()
    }
}

impl PartialOrd for Value {
    fn partial_cmp(&self, other: &Value) -> Option<Ordering> {
        if let Ok(r) = self.compare(other, 0) {
            Some(r)
        } else {
            None
        }
    }
}

impl dyn TypedValue {
    // To be calleds by convert_slice_indices only
    fn convert_index_aux(
        len: i64,
        v1: Option<Value>,
        default: i64,
        min: i64,
        max: i64,
    ) -> Result<i64, ValueError> {
        if let Some(v) = v1 {
            if v.get_type() == "NoneType" {
                Ok(default)
            } else {
                match v.to_int() {
                    Ok(x) => {
                        let i = if x < 0 { len + x } else { x };
                        if i < min {
                            Ok(min)
                        } else if i > max {
                            Ok(max)
                        } else {
                            Ok(i)
                        }
                    }
                    Err(..) => Err(ValueError::IncorrectParameterType),
                }
            }
        } else {
            Ok(default)
        }
    }

    /// Macro to parse the index for at/set_at methods.
    ///
    /// Return an `i64` from self corresponding to the index recenterd between 0 and len.
    /// Raise the correct errors if the value is not numeric or the index is out of bound.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # use starlark::values::*;
    /// # assert!(
    /// Value::new(6).convert_index(7).unwrap() == 6
    /// # );
    /// # assert!(
    /// Value::new(-1).convert_index(7).unwrap() == 6
    /// # );
    /// ```
    ///
    /// The following examples would return an error
    /// ```rust
    /// # use starlark::values::*;
    /// # use starlark::values::error::*;
    /// # use starlark::values::string;
    /// # assert!(
    /// Value::from("a").convert_index(7) == Err(ValueError::IncorrectParameterType)
    /// # );
    /// # assert!(
    /// Value::new(8).convert_index(7) == Err(ValueError::IndexOutOfBound(8))   // 8 > 7 = len
    /// # );
    /// # assert!(
    /// Value::new(-8).convert_index(7) == Err(ValueError::IndexOutOfBound(-1)) // -8 + 7 = -1 < 0
    /// # );
    /// ```
    pub fn convert_index(&self, len: i64) -> Result<i64, ValueError> {
        match self.to_int() {
            Ok(x) => {
                let i = if x < 0 { len + x } else { x };
                if i < 0 || i >= len {
                    Err(ValueError::IndexOutOfBound(i))
                } else {
                    Ok(i)
                }
            }
            Err(..) => Err(ValueError::IncorrectParameterType),
        }
    }

    /// Parse indices for slicing.
    ///
    /// Takes the object length and 3 optional values and returns `(i64, i64, i64)`
    /// with those index correctly converted in range of length.
    /// Return the correct errors if the values are not numeric or the stride is 0.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # use starlark::values::*;
    /// let six      = Some(Value::new(6));
    /// let minusone = Some(Value::new(-1));
    /// let ten      = Some(Value::new(10));
    ///
    /// # assert!(
    /// TypedValue::convert_slice_indices(7, six, None, None).unwrap() == (6, 7, 1)
    /// # );
    /// # assert!(
    /// TypedValue::convert_slice_indices(7, minusone.clone(), None, minusone.clone()).unwrap()
    ///        == (6, -1, -1)
    /// # );
    /// # assert!(
    /// TypedValue::convert_slice_indices(7, minusone, ten, None).unwrap() == (6, 7, 1)
    /// # );
    /// ```
    pub fn convert_slice_indices(
        len: i64,
        start: Option<Value>,
        stop: Option<Value>,
        stride: Option<Value>,
    ) -> Result<(i64, i64, i64), ValueError> {
        let stride = stride.unwrap_or_else(|| Value::new(1));
        let stride = if stride.get_type() == "NoneType" {
            Ok(1)
        } else {
            stride.to_int()
        };
        match stride {
            Ok(0) => Err(ValueError::IndexOutOfBound(0)),
            Ok(stride) => {
                let def_start = if stride < 0 { len - 1 } else { 0 };
                let def_end = if stride < 0 { -1 } else { len };
                let clamp = if stride < 0 { -1 } else { 0 };
                let start =
                    TypedValue::convert_index_aux(len, start, def_start, clamp, len + clamp);
                let stop = TypedValue::convert_index_aux(len, stop, def_end, clamp, len + clamp);
                match (start, stop) {
                    (Ok(s1), Ok(s2)) => Ok((s1, s2, stride)),
                    (Err(x), ..) => Err(x),
                    (Ok(..), Err(x)) => Err(x),
                }
            }
            _ => Err(ValueError::IncorrectParameterType),
        }
    }
}

impl Value {
    /// A convenient wrapper around any_apply to actually operate on the underlying type
    pub fn downcast_apply<T: Any + TypedValue, F, Return>(&self, f: F) -> Option<Return>
    where
        F: FnOnce(&T) -> Return,
    {
        self.any_apply(move |x| match x.downcast_ref() {
            Some(x) => Some(f(x)),
            None => None,
        })
    }

    /// A convenient wrapper around any_apply_mut to actually operate on the underlying type
    pub fn downcast_apply_mut<T: Any + TypedValue, F, Return>(&mut self, f: F) -> Option<Return>
    where
        F: FnOnce(&mut T) -> Return,
    {
        self.any_apply_mut(move |x| match x.downcast_mut() {
            Some(x) => Some(f(x)),
            None => None,
        })
    }

    pub fn convert_index(&self, len: i64) -> Result<i64, ValueError> {
        let borrowed = self.0.borrow();
        borrowed.convert_index(len)
    }

    pub fn convert_slice_indices(
        len: i64,
        start: Option<Value>,
        stop: Option<Value>,
        stride: Option<Value>,
    ) -> Result<(i64, i64, i64), ValueError> {
        TypedValue::convert_slice_indices(len, start, stop, stride)
    }
}

// Submodules
pub mod boolean;
pub mod dict;
pub mod error;
pub mod function;
pub mod hashed_value;
pub mod int;
pub mod list;
pub mod none;
pub mod string;
pub mod tuple;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_convert_index() {
        assert_eq!(Ok(6), Value::new(6).convert_index(7));
        assert_eq!(Ok(6), Value::new(-1).convert_index(7));
        assert_eq!(
            Ok((6, 7, 1)),
            TypedValue::convert_slice_indices(7, Some(Value::new(6)), None, None)
        );
        assert_eq!(
            Ok((6, -1, -1)),
            TypedValue::convert_slice_indices(7, Some(Value::new(-1)), None, Some(Value::new(-1)))
        );
        assert_eq!(
            Ok((6, 7, 1)),
            TypedValue::convert_slice_indices(7, Some(Value::new(-1)), Some(Value::new(10)), None)
        );
        // Errors
        assert_eq!(
            Err(ValueError::IncorrectParameterType),
            Value::from("a").convert_index(7)
        );
        assert_eq!(
            Err(ValueError::IndexOutOfBound(8)),
            Value::new(8).convert_index(7)
        );
        assert_eq!(
            Err(ValueError::IndexOutOfBound(-1)),
            Value::new(-8).convert_index(7)
        );
    }

    #[test]
    fn can_implement_compare() {
        #[derive(Debug, PartialEq, Eq, Ord, PartialOrd)]
        struct WrappedNumber(u64);

        /// Define the NoneType type
        impl TypedValue for WrappedNumber {
            immutable!();
            any!();
            fn to_repr(&self) -> String {
                format!("{:?}", self)
            }
            fn get_type(&self) -> &'static str {
                "WrappedNumber"
            }
            fn to_bool(&self) -> bool {
                false
            }
            fn get_hash(&self) -> Result<u64, ValueError> {
                Ok(self.0)
            }
            fn compare<'a>(
                &'a self,
                other: &dyn TypedValue,
                _recursion: u32,
            ) -> Result<std::cmp::Ordering, ValueError> {
                match other.get_type() {
                    "WrappedNumber" => {
                        let other = other.as_any().downcast_ref::<Self>().unwrap();
                        Ok(std::cmp::Ord::cmp(self, other))
                    }
                    _ => default_compare(self, other),
                }
            }

            fn is_descendant(&self, _other: &TypedValue) -> bool {
                false
            }
        }

        let one = Value::new(WrappedNumber(1));
        let another_one = Value::new(WrappedNumber(1));
        let two = Value::new(WrappedNumber(2));
        let not_wrapped_number: Value = 1.into();

        use std::cmp::Ordering::*;

        assert_eq!(one.compare(&one, 0), Ok(Equal));
        assert_eq!(one.compare(&another_one, 0), Ok(Equal));
        assert_eq!(one.compare(&two, 0), Ok(Less));
        assert_eq!(two.compare(&one, 0), Ok(Greater));
        assert_eq!(one.compare(&not_wrapped_number, 0), Ok(Greater));
        assert_eq!(not_wrapped_number.compare(&one, 0), Ok(Less));
    }
}
