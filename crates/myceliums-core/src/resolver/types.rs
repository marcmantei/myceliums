//! Receiver typing: what a call's receiver is, from declarations in view.
//!
//! No type inference. A receiver's type is known when the code says it:
//!
//! - `self`, `this`, `super`, Rust's `Self`: the enclosing class (its base);
//! - a class or module name: `Config.load()`, `Validator::new()`,
//!   `config.parse()`;
//! - a constructor or literal: `new Config(...)`, `User(...)`,
//!   `&model.User{...}`, a string;
//! - a parameter, variable or field with a declared type (`repo:
//!   Repository`, `Repository<User> repo`, `repo *store.Repo[*User]`), or
//!   initialised from one of the above (`user = User(...)`);
//! - a call's declared return type (`Config.load(text): Config`, `-> Self`,
//!   `func Load(...) (*Config, error)`).
//!
//! Members are looked up in the class and its base classes (Go: embedded
//! structs). Once a receiver is typed, a member its class does not declare
//! (as a method or a field, here or in a project base class) comes from a
//! library base class or is dynamic: [`Ty::External`], never a match by
//! name. A declared field whose value has no visible type stays
//! [`Ty::Unknown`], as does the result of a project function without a
//! declared return type; only then does the resolver fall back to matching
//! the method name.

use super::imports::Bound;
use super::{implicit_this, package_scope, CallResolver, Def, DefId, Site};
use crate::parser::{BindingValue, ExpressionStep, TypeBinding};
use myceliums_storage::SymbolKind;
use std::cell::RefCell;
use std::collections::HashMap;

/// Memoised base classes and open hierarchies, per (class, depth). Every
/// lookup they make goes deeper, so a memoised answer is the one the
/// lookup would compute again; the resolver clears the memo whenever it
/// re-prepares. Without it Django's library-member calls resolve the same
/// bases tens of thousands of times.
#[derive(Default)]
pub(super) struct BaseMemo {
    kinds: RefCell<HashMap<(DefId, u32), Vec<BaseKind>>>,
    open: RefCell<HashMap<(DefId, u32), bool>>,
}

/// What a declared base class resolves to.
#[derive(Clone, Copy)]
enum BaseKind {
    /// A project class, interface or trait.
    Project(DefId),
    /// Something outside the project.
    Library,
    /// A project name the resolver cannot pin down.
    Unresolved,
}

/// How deep receiver typing follows bindings, return types and base
/// classes.
const MAX_DEPTH: u32 = 8;

/// What an expression is.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Ty {
    /// An instance of a project class.
    Instance(DefId),
    /// A project class itself: static members, a constructor call.
    Class(DefId),
    /// A module or package: its files.
    Module(Vec<String>),
    /// A project function or method, not yet called.
    Function(DefId),
    /// A value of a type from outside the project (a literal, a library
    /// type, an array).
    External,
    /// Not known.
    Unknown,
}

impl CallResolver {
    /// The type of a chain of steps evaluated at `site`.
    pub(super) fn eval(&self, site: &Site, steps: &[ExpressionStep], depth: u32) -> Ty {
        if depth > MAX_DEPTH {
            return Ty::Unknown;
        }
        let Some((first, mut rest)) = steps.split_first() else {
            return Ty::Unknown;
        };
        // Python's `super()` is the base-class instance, not a call result.
        if *first == ExpressionStep::Super {
            rest = rest.strip_prefix(&[ExpressionStep::Call]).unwrap_or(rest);
        }
        let mut ty = match first {
            ExpressionStep::This => match self.enclosing_class(site) {
                Some(class) => Ty::Instance(class),
                None => Ty::Unknown,
            },
            // A class with no project base class inherits from a library
            // class (or the language's root object).
            ExpressionStep::Super => match self.enclosing_class(site) {
                Some(class) => match self.bases(class, depth).first() {
                    Some(&base) => Ty::Instance(base),
                    None => Ty::External,
                },
                None => Ty::Unknown,
            },
            ExpressionStep::New(name) => self.type_ty(site, name, depth + 1),
            ExpressionStep::Literal => Ty::External,
            ExpressionStep::Ident(name) => {
                let called = rest.first() == Some(&ExpressionStep::Call);
                self.ident_ty(site, name, called, depth + 1)
            }
            ExpressionStep::Field(_) | ExpressionStep::Call | ExpressionStep::Index => Ty::Unknown,
        };
        let methods_apart = super::separate_method_namespace(site.file);
        for (i, step) in rest.iter().enumerate() {
            // `x.name()` in Java is a method even where x has a field `name`.
            let method = methods_apart && rest.get(i + 1) == Some(&ExpressionStep::Call);
            ty = match (ty, step) {
                (Ty::External, _) => return Ty::External,
                (Ty::Unknown, _) => return Ty::Unknown,
                (Ty::Instance(class), ExpressionStep::Field(name)) if method => {
                    self.member_value(class, name, depth + 1)
                }
                (Ty::Instance(class), ExpressionStep::Field(name)) => {
                    match self.field_ty(class, name, depth + 1) {
                        Some(ty) => ty,
                        None => self.member_value(class, name, depth + 1),
                    }
                }
                (Ty::Class(class), ExpressionStep::Field(name)) => {
                    match self.members_of(class, name).as_slice() {
                        [member] if self.defs[*member].is_class_like() => Ty::Class(*member),
                        [] if method => self.member_value(class, name, depth + 1),
                        [] => self
                            .field_ty(class, name, depth + 1)
                            .unwrap_or_else(|| self.member_value(class, name, depth + 1)),
                        members => self.method_value(members),
                    }
                }
                (Ty::Module(files), ExpressionStep::Field(name)) => {
                    match self.module_member(&files, site.family, name) {
                        Some(bound) => self.bound_ty(bound, depth + 1),
                        // The project module declares no such name.
                        None => Ty::External,
                    }
                }
                (Ty::Class(class), ExpressionStep::Call) => Ty::Instance(class),
                (Ty::Function(function), ExpressionStep::Call) => {
                    self.return_ty(function, depth + 1)
                }
                // Calling an instance, indexing a typed value, a member of a
                // function: whatever it is, it is not a project type.
                _ => Ty::External,
            };
        }
        ty
    }

    /// Member `name` of `class` as a value: its method; unknown when the
    /// class does not declare it but may get it from a project base the
    /// resolver cannot see; else a library member.
    fn member_value(&self, class: DefId, name: &str, depth: u32) -> Ty {
        let methods = self.methods_on(class, name, depth);
        if methods.is_empty() && self.member_may_be_project(class, name, depth) {
            return Ty::Unknown;
        }
        self.method_value(&methods)
    }

    /// A method used as a value (about to be called): the method, or for
    /// several overloads one whose return type they all share. None at all:
    /// the member comes from a library base class.
    fn method_value(&self, methods: &[DefId]) -> Ty {
        match methods {
            [] => Ty::External,
            [first, rest @ ..] => {
                let returns = |id: &DefId| self.defs[*id].return_type.as_deref();
                if rest.iter().all(|m| returns(m) == returns(first)) {
                    Ty::Function(*first)
                } else {
                    Ty::External
                }
            }
        }
    }

    /// What an import or module member binds, as a type.
    fn bound_ty(&self, bound: Bound, depth: u32) -> Ty {
        match bound {
            Bound::Defs(defs) => match defs.as_slice() {
                [def] => self.def_ty(*def, depth),
                _ => Ty::Unknown,
            },
            Bound::Module(files) => Ty::Module(files),
            Bound::Value(file, index) => match self.files.get(&file) {
                Some(context) => self.binding_ty(&file, index, &context.bindings[index], depth),
                None => Ty::Unknown,
            },
            Bound::Unknown => Ty::Unknown,
            Bound::External => Ty::External,
        }
    }

    /// A definition used as a value: a class, a function, or a module-level
    /// variable or constant with the type its declaration gives.
    fn def_ty(&self, def: DefId, depth: u32) -> Ty {
        let d = &self.defs[def];
        if d.is_class_like() {
            return Ty::Class(def);
        }
        if d.is_function_like() {
            return Ty::Function(def);
        }
        match self.module_value(&d.file, &d.name) {
            Some(index) => self.bound_ty(Bound::Value(d.file.clone(), index), depth),
            None => Ty::Unknown,
        }
    }

    /// What a name is at `site`: a binding (which shadows everything else),
    /// a field or method of the enclosing class (Java), a class, an import, a
    /// function in scope. In a language whose bindings are extracted, a name
    /// none of these knows is a global or built-in: [`Ty::External`].
    ///
    /// `called`: the name is called (`name()`). In Java a call never reaches
    /// a variable or field, only a method.
    fn ident_ty(&self, site: &Site, name: &str, called: bool, depth: u32) -> Ty {
        if name == "Self" {
            return self.enclosing_class(site).map_or(Ty::Unknown, Ty::Class);
        }
        let values = !(called && super::separate_method_namespace(site.file));
        if let Some((file, index, binding)) = self.binding(site, name).filter(|_| values) {
            return self.binding_ty(file, index, binding, depth);
        }
        if implicit_this(site.family) {
            for class in self.enclosing_classes(site).into_iter().filter(|_| values) {
                if let Some(ty) = self.field_ty(class, name, depth) {
                    return ty;
                }
            }
            let members: Vec<DefId> = self
                .enclosing_members(site, name, depth)
                .into_iter()
                .filter(|&id| self.defs[id].is_function_like())
                .collect();
            if let [member] = members.as_slice() {
                return Ty::Function(*member);
            }
        }
        match self.classes_in_scope(site, name).as_slice() {
            [class] => return Ty::Class(*class),
            [] => {}
            _ => return Ty::Unknown,
        }
        if let Some(bound) = self.imported(site, name) {
            return self.bound_ty(bound, depth);
        }
        let mut functions = self.in_file(site, name, Def::is_function_like);
        if functions.is_empty() && package_scope(site.family) {
            functions = self.in_package(site, name, |d| {
                d.is_function_like() && d.container.is_none()
            });
        }
        match functions.as_slice() {
            [function] => return Ty::Function(*function),
            [] => {}
            _ => return Ty::Unknown,
        }
        // A module-level variable or constant (a Rust `static`, a Go package
        // variable in another file): the type its declaration gives, if any.
        let is_value = |d: &Def| matches!(d.kind, SymbolKind::Variable | SymbolKind::Constant);
        let mut values = self.in_file(site, name, is_value);
        if values.is_empty() && package_scope(site.family) {
            values = self.in_package(site, name, is_value);
        }
        match values.as_slice() {
            [value] => self.def_ty(*value, depth),
            [_, ..] => Ty::Unknown,
            [] if self.binds_names(site.family) => Ty::External,
            [] => Ty::Unknown,
        }
    }

    /// Families whose extractor reports parameters and locals: a name
    /// bound nowhere is a global or a built-in.
    fn binds_names(&self, family: &str) -> bool {
        matches!(family, "python" | "js" | "go" | "rs" | "jvm")
    }

    /// The binding of `name` visible at `site` (a parameter or variable):
    /// the most recently declared one whose scope contains the line. An
    /// export alias binds nothing inside its module.
    pub(super) fn binding<'a>(
        &'a self,
        site: &Site<'a>,
        name: &str,
    ) -> Option<(&'a str, usize, &'a TypeBinding)> {
        let context = site.context?;
        context
            .bindings
            .iter()
            .enumerate()
            .filter(|(i, b)| {
                b.owner.is_none()
                    && !matches!(b.value, BindingValue::Export(_))
                    && b.name == name
                    && b.scope.0 <= site.line
                    && site.line <= b.scope.1
                    && Some(*i) != site.exclude
            })
            .max_by_key(|(i, b)| (b.scope.0, *i))
            .map(|(i, b)| (site.file, i, b))
    }

    /// The type a binding declares or is initialised to, evaluated where it
    /// is declared.
    fn binding_ty(&self, file: &str, index: usize, binding: &TypeBinding, depth: u32) -> Ty {
        if depth > MAX_DEPTH {
            return Ty::Unknown;
        }
        let Some(mut site) = self.site_at(file, binding.scope.0) else {
            return Ty::Unknown;
        };
        site.exclude = Some(index);
        match &binding.value {
            BindingValue::Type(ty) => self.type_ty(&site, ty, depth + 1),
            BindingValue::Expr(steps) => self.eval(&site, steps, depth + 1),
            BindingValue::Unknown => Ty::Unknown,
            BindingValue::Export(name) => {
                self.eval(&site, &[ExpressionStep::Ident(name.clone())], depth + 1)
            }
        }
    }

    /// The type of field `name` of `class` or of one of its bases;
    /// `Some(Unknown)` when it is declared or assigned without a visible
    /// type, `None` when no project class in the hierarchy has it.
    ///
    /// Fields live in the class's file or, for Go and Rust, another file of
    /// its directory; those are taken from the path index in sorted order,
    /// so the lookup is deterministic and does not scan every file.
    fn field_ty(&self, class: DefId, name: &str, depth: u32) -> Option<Ty> {
        if depth > MAX_DEPTH {
            return None;
        }
        let def = &self.defs[class];
        let siblings = self.index.files_in(def.dir(), &def.family);
        let files = std::iter::once(def.file.as_str()).chain(
            siblings
                .iter()
                .map(String::as_str)
                .filter(|f| *f != def.file),
        );
        let mut untyped = false;
        for file in files {
            let Some(context) = self.files.get(file) else {
                continue;
            };
            let found = context
                .bindings
                .iter()
                .enumerate()
                .rev()
                .find(|(_, b)| b.name == name && b.owner.as_deref() == Some(def.name.as_str()));
            if let Some((index, binding)) = found {
                // A field assigned from an unknown value may still be
                // declared with a type elsewhere: keep looking.
                match self.binding_ty(file, index, binding, depth + 1) {
                    Ty::Unknown => untyped = true,
                    ty => return Some(ty),
                }
            }
        }
        self.bases(class, depth)
            .into_iter()
            .find_map(|base| self.field_ty(base, name, depth + 1))
            .or(untyped.then_some(Ty::Unknown))
    }

    /// What calling `function` returns, from its declared return type.
    fn return_ty(&self, function: DefId, depth: u32) -> Ty {
        let def = &self.defs[function];
        let Some(declared) = def.return_type.as_deref() else {
            return Ty::Unknown;
        };
        let Some(site) = self.site_at(&def.file, def.start) else {
            return Ty::Unknown;
        };
        // `-> Self` in an impl, `-> &Self`
        if type_name(&def.family, declared).as_deref() == Some("Self") {
            return self
                .container_class(function)
                .map_or(Ty::Unknown, Ty::Instance);
        }
        self.type_ty(&site, declared, depth)
    }

    /// The instance type a type annotation names, resolved at `site`.
    pub(super) fn type_ty(&self, site: &Site, declared: &str, depth: u32) -> Ty {
        if depth > MAX_DEPTH {
            return Ty::Unknown;
        }
        let Some(name) = type_name(site.family, declared) else {
            return Ty::External;
        };
        if name == "Self" {
            return self.enclosing_class(site).map_or(Ty::Unknown, Ty::Instance);
        }
        let segments: Vec<&str> = name
            .split("::")
            .flat_map(|s| s.split('.'))
            .filter(|s| !s.is_empty() && !matches!(*s, "crate" | "self" | "super"))
            .collect();
        let steps: Vec<ExpressionStep> = segments
            .iter()
            .enumerate()
            .map(|(i, s)| {
                if i == 0 {
                    ExpressionStep::Ident(s.to_string())
                } else {
                    ExpressionStep::Field(s.to_string())
                }
            })
            .collect();
        let ty = if segments.len() == 1 {
            match self.classes_in_scope(site, segments[0]).as_slice() {
                [class] => Ty::Class(*class),
                [] => match self.imported(site, segments[0]) {
                    Some(Bound::Defs(defs)) => match defs.as_slice() {
                        [def] if self.defs[*def].is_class_like() => Ty::Class(*def),
                        _ => Ty::Unknown,
                    },
                    Some(Bound::External) => Ty::External,
                    // A module-level alias of a class (`Base = Model`,
                    // `export { Hono as HonoBase }`).
                    Some(value @ Bound::Value(..)) => match self.bound_ty(value, depth + 1) {
                        Ty::Class(class) => Ty::Class(class),
                        _ => Ty::Unknown,
                    },
                    Some(_) => Ty::Unknown,
                    None if self.binds_names(site.family) => Ty::External,
                    None => Ty::Unknown,
                },
                _ => Ty::Unknown,
            }
        } else {
            self.eval(site, &steps, depth + 1)
        };
        match ty {
            Ty::Class(class) => Ty::Instance(class),
            Ty::External => Ty::External,
            _ => Ty::Unknown,
        }
    }

    /// The class whose method or body the site is in.
    pub(super) fn enclosing_class(&self, site: &Site) -> Option<DefId> {
        if let Some(class) = site.caller.and_then(|caller| self.container_class(caller)) {
            return Some(class);
        }
        self.enclosing_classes(site).first().copied()
    }

    /// The class a method belongs to: the class-like definition named like
    /// its container, in its file, its directory, or (Rust `impl` blocks)
    /// anywhere in its language family when only one has that name.
    pub(super) fn container_class(&self, member: DefId) -> Option<DefId> {
        let def = &self.defs[member];
        let container = def.container.as_deref()?;
        let name = container.rsplit('.').next()?;
        let classes: Vec<DefId> = self
            .named(name)
            .filter(|&id| self.defs[id].is_class_like() && self.defs[id].family == def.family)
            .collect();
        let in_file: Vec<DefId> = classes
            .iter()
            .copied()
            .filter(|&id| self.defs[id].file == def.file)
            .collect();
        // Several classes of that name in the file (nested): the innermost
        // one around the member.
        if let Some(&class) = in_file
            .iter()
            .filter(|&&id| self.defs[id].start <= def.start && def.end <= self.defs[id].end)
            .min_by_key(|&&id| self.defs[id].end - self.defs[id].start)
            .or(in_file.first())
        {
            return Some(class);
        }
        let in_dir: Vec<DefId> = classes
            .iter()
            .copied()
            .filter(|&id| self.defs[id].dir() == def.dir())
            .collect();
        match (in_dir.as_slice(), classes.as_slice()) {
            ([class], _) | ([], [class]) => Some(*class),
            _ => None,
        }
    }

    /// The base classes of `class` that are project classes (Go: embedded
    /// structs; Java: also implemented interfaces; Rust: implemented traits),
    /// each resolved where it is declared.
    pub(super) fn bases(&self, class: DefId, depth: u32) -> Vec<DefId> {
        self.base_kinds(class, depth)
            .into_iter()
            .filter_map(|kind| match kind {
                BaseKind::Project(base) => Some(base),
                _ => None,
            })
            .collect()
    }

    /// What each declared base of `class` is.
    fn base_kinds(&self, class: DefId, depth: u32) -> Vec<BaseKind> {
        if depth > MAX_DEPTH {
            return Vec::new();
        }
        if let Some(kinds) = self.base_memo.kinds.borrow().get(&(class, depth)) {
            return kinds.clone();
        }
        let def = &self.defs[class];
        let kinds: Vec<BaseKind> = def
            .bases
            .iter()
            .map(|base| {
                let (file, line) = match &base.at {
                    Some((file, line)) => (file.as_str(), *line),
                    None => (def.file.as_str(), def.start),
                };
                match self.site_at(file, line) {
                    Some(site) => self.base_kind(&site, &base.text, class, depth),
                    None => BaseKind::Unresolved,
                }
            })
            .collect();
        self.base_memo
            .kinds
            .borrow_mut()
            .insert((class, depth), kinds.clone());
        kinds
    }

    /// A base resolves to a project class, to something outside the
    /// project (imported from a library, or a built-in / prelude name bound
    /// nowhere: `Exception`, `Iterator`), or to neither: a project name the
    /// resolver cannot pin down (a module-level value, a name a project
    /// module exports invisibly, an ambiguous class name, a crate path it
    /// cannot follow, a base computed by a call).
    fn base_kind(&self, site: &Site, text: &str, class: DefId, depth: u32) -> BaseKind {
        if text.contains('(') {
            return BaseKind::Unresolved;
        }
        match self.type_ty(site, text, depth + 1) {
            Ty::Instance(base) if base != class => return BaseKind::Project(base),
            Ty::Instance(_) => return BaseKind::Library,
            _ => {}
        }
        let Some(name) = type_name(site.family, text) else {
            return BaseKind::Library;
        };
        if ["crate::", "self::", "super::"]
            .iter()
            .any(|prefix| name.starts_with(prefix))
        {
            return BaseKind::Unresolved;
        }
        let Some(first) = name
            .split("::")
            .flat_map(|s| s.split('.'))
            .find(|s| !s.is_empty())
        else {
            return BaseKind::Unresolved;
        };
        if self.binding(site, first).is_some() || self.classes_in_scope(site, first).len() > 1 {
            return BaseKind::Unresolved;
        }
        match self.imported(site, first) {
            Some(Bound::External) | None => BaseKind::Library,
            Some(_) => BaseKind::Unresolved,
        }
    }

    /// Whether a member `name` that `class` and its project bases do not
    /// declare may still be a project method: when a base in the hierarchy
    /// is unresolved, or (Rust) a project trait declares a method of that
    /// name — the type may implement it where the resolver did not look.
    fn member_may_be_project(&self, class: DefId, name: &str, depth: u32) -> bool {
        self.open_hierarchy(class, depth)
            || (self.defs[class].family == "rs" && !self.trait_methods("rs", name).is_empty())
    }

    /// Whether `class` or one of its project bases has an unresolved base.
    pub(super) fn open_hierarchy_of(&self, class: DefId) -> bool {
        self.open_hierarchy(class, 0)
    }

    fn open_hierarchy(&self, class: DefId, depth: u32) -> bool {
        if depth > MAX_DEPTH {
            return false;
        }
        if let Some(&open) = self.base_memo.open.borrow().get(&(class, depth)) {
            return open;
        }
        let open = self
            .base_kinds(class, depth)
            .into_iter()
            .any(|kind| match kind {
                BaseKind::Unresolved => true,
                BaseKind::Project(base) => self.open_hierarchy(base, depth + 1),
                BaseKind::Library => false,
            });
        self.base_memo
            .open
            .borrow_mut()
            .insert((class, depth), open);
        open
    }

    /// Methods named `name` that a trait (or interface) of `family`
    /// declares.
    pub(super) fn trait_methods(&self, family: &str, name: &str) -> Vec<DefId> {
        self.named(name)
            .filter(|&id| {
                let def = &self.defs[id];
                def.family == family
                    && def.is_function_like()
                    && self
                        .container_class(id)
                        .is_some_and(|c| self.defs[c].kind == SymbolKind::Interface)
            })
            .collect()
    }

    /// Methods (and nested classes) named `name` of `class`, else of its
    /// nearest base classes that have one.
    pub(super) fn methods_on(&self, class: DefId, name: &str, depth: u32) -> Vec<DefId> {
        if depth > MAX_DEPTH {
            return Vec::new();
        }
        let own = self.members_of(class, name);
        if !own.is_empty() {
            return own;
        }
        self.bases(class, depth)
            .into_iter()
            .map(|base| self.methods_on(base, name, depth + 1))
            .find(|found| !found.is_empty())
            .unwrap_or_default()
    }

    /// A site at `file:line`, for evaluating a declaration made there.
    pub(super) fn site_at<'a>(&'a self, file: &'a str, line: u32) -> Option<Site<'a>> {
        let family = super::language_family(file)?;
        let caller = self.by_file.get(file).and_then(|ids| {
            ids.iter()
                .copied()
                .filter(|&id| self.defs[id].is_function_like() && self.defs[id].contains(line))
                .min_by_key(|&id| self.defs[id].end - self.defs[id].start)
        });
        Some(Site {
            file,
            family,
            line,
            caller,
            context: self.files.get(file),
            exclude: None,
        })
    }
}

/// The class a type annotation names, without references, pointers,
/// generic arguments or nullability: `&mut Repo<User>` → `Repo`,
/// `*store.Repo[*model.User]` → `store.Repo`, `Token | null` → `Token`,
/// `(*Config, error)` → `Config`, `Optional["Config"]` → `Config`. `None`
/// for an array, slice, map, tuple or function type: never a project class.
pub(super) fn type_name(family: &str, declared: &str) -> Option<String> {
    let mut text = declared.trim();
    // A Go result list: the first result.
    if family == "go" && text.starts_with('(') && text.ends_with(')') {
        text = text[1..text.len() - 1].split(',').next()?.trim();
        // `(cfg *Config, err error)`: named results.
        if let Some((_, ty)) = text.split_once(' ') {
            text = ty.trim();
        }
    }
    // A union: its first member that is not null.
    if text.contains('|') && family != "rs" {
        text = text
            .split('|')
            .map(str::trim)
            .find(|t| !matches!(*t, "null" | "undefined" | "None" | "nil" | ""))?;
    }
    let text = text.trim_matches(['"', '\'']);
    let mut text = text.trim();
    loop {
        let before = text;
        for prefix in [
            "&mut ",
            "&",
            "mut ",
            "dyn ",
            "impl ",
            "*",
            "readonly ",
            "const ",
            "final ",
        ] {
            text = text.strip_prefix(prefix).unwrap_or(text).trim_start();
        }
        // A lifetime: `&'a T`
        if let Some(rest) = text.strip_prefix('\'') {
            text = rest.split_once(' ').map_or("", |(_, t)| t).trim_start();
        }
        if text == before {
            break;
        }
    }
    if family == "python" {
        if let Some(inner) = text
            .strip_prefix("Optional[")
            .and_then(|t| t.strip_suffix(']'))
        {
            return type_name(family, inner);
        }
    }
    if text.starts_with('[')
        || text.starts_with('(')
        || text.starts_with('{')
        || text.ends_with("[]")
        || text.starts_with("map[")
        || text.starts_with("chan ")
        || text.starts_with("func")
        || text.starts_with("fn(")
    {
        return None;
    }
    let generic = if matches!(family, "go" | "python") {
        '['
    } else {
        '<'
    };
    let text = text
        .split(generic)
        .next()?
        .trim()
        .trim_end_matches('?')
        .trim();
    let is_path = !text.is_empty()
        && text
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '_' | '.' | ':' | '$'));
    is_path.then(|| text.to_string())
}

#[cfg(test)]
mod tests {
    use super::type_name;

    #[test]
    fn type_names_drop_references_generics_and_nullability() {
        let name = |family, text| type_name(family, text);
        assert_eq!(name("rs", "&mut Repo<User>").as_deref(), Some("Repo"));
        assert_eq!(name("rs", "&'a dyn Sink").as_deref(), Some("Sink"));
        assert_eq!(name("rs", "Self").as_deref(), Some("Self"));
        assert_eq!(name("rs", "&[String]"), None);
        assert_eq!(
            name("go", "*store.Repo[*model.User]").as_deref(),
            Some("store.Repo")
        );
        assert_eq!(name("go", "(*Config, error)").as_deref(), Some("Config"));
        assert_eq!(name("go", "[]*model.Order"), None);
        assert_eq!(name("js", "Token | null").as_deref(), Some("Token"));
        assert_eq!(
            name("js", "Repository<User>").as_deref(),
            Some("Repository")
        );
        assert_eq!(name("js", "string[]"), None);
        assert_eq!(name("jvm", "Map<String, String>").as_deref(), Some("Map"));
        assert_eq!(
            name("python", "Optional['Config']").as_deref(),
            Some("Config")
        );
        assert_eq!(name("python", "list[int]").as_deref(), Some("list"));
    }
}
