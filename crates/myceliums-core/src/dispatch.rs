//! Dynamic dispatch (#190): which implementations a call through an
//! interface, abstract or overridden method may run.
//!
//! The analyzer records one OVERRIDES edge from each method to every
//! base-class or interface method it overrides or implements, through the
//! whole hierarchy ([`crate::CallResolver::resolve_overrides`]). CALLS edges
//! keep linking a call to the declaration the receiver's static type binds;
//! dispatch is derived from the two at query time and never written as a
//! CALLS edge:
//!
//! - the *callers* of an implementation include the callers of each method
//!   it overrides (`find_callers`, `impact_of_change`);
//! - the *callees* of a call to an overridden method include its known
//!   implementations, at most [`MAX_LISTED_IMPLEMENTATIONS`] per call
//!   (`find_callees`, `trace_flow`).
//!
//! Each OVERRIDES edge carries a [`DispatchConfidence`]: how likely a call
//! bound to the overridden method runs this implementation. Overrides of
//! the methods every object has (`toString`, `equals`, `hashCode`, ...) are
//! recorded but never followed: a call of `equals` reaches almost anything.

use std::collections::HashMap;

use myceliums_storage::{CodeSymbol, Relationship, RelationshipKind};

/// Most implementations a callee list shows for one call; the rest are
/// counted (see [`DispatchCut`]).
pub const MAX_LISTED_IMPLEMENTATIONS: usize = 8;

/// More overriding implementations than this make a method *wide*: a call
/// through it says little about which one runs.
pub const WIDE_DISPATCH: usize = 16;

/// What the overridden method is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverriddenKind {
    /// An interface or abstract method: every call runs some override.
    Abstract,
    /// A method with a body of its own: a call may run it instead.
    Concrete,
}

/// What one OVERRIDES edge records (in its metadata).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DispatchConfidence {
    /// What the overridden method is.
    pub overridden: OverriddenKind,
    /// Concrete implementations of the overridden method in the project.
    pub implementations: usize,
    /// The overriding method has no body (an interface re-declaring it, an
    /// abstract re-declaration): it never runs, so callee lists skip it.
    pub abstract_override: bool,
    /// A method every object has (`toString`, `equals`, ...): never followed.
    pub universal: bool,
    /// Several overloads of the base class fit the overriding method and
    /// the parameter types do not tell which it overrides: an edge to each,
    /// at most [`AMBIGUOUS_CONFIDENCE`].
    pub ambiguous: bool,
}

/// The highest confidence of an ambiguous OVERRIDES edge.
pub const AMBIGUOUS_CONFIDENCE: f32 = 0.2;

impl DispatchConfidence {
    /// How likely a call bound to the overridden method runs this
    /// implementation, from 0.1 to 0.9: by the number of implementations
    /// (1, 2-3, 4-8, 9-16, more), lower when the overridden method has a
    /// body of its own.
    ///
    /// | implementations | 1 | 2-3 | 4-8 | 9-16 | more |
    /// |---|---:|---:|---:|---:|---:|
    /// | interface / abstract | 0.9 | 0.8 | 0.6 | 0.4 | 0.2 |
    /// | method with a body | 0.7 | 0.6 | 0.5 | 0.3 | 0.1 |
    ///
    /// A universal method (`toString`, ...) is 0.1; an ambiguous edge at
    /// most [`AMBIGUOUS_CONFIDENCE`].
    pub fn confidence(&self) -> f32 {
        if self.universal {
            return 0.1;
        }
        let tier = match self.implementations {
            0 | 1 => 0,
            2..=3 => 1,
            4..=8 => 2,
            n if n <= WIDE_DISPATCH => 3,
            _ => 4,
        };
        let confidence: f32 = match self.overridden {
            OverriddenKind::Abstract => [0.9, 0.8, 0.6, 0.4, 0.2][tier],
            OverriddenKind::Concrete => [0.7, 0.6, 0.5, 0.3, 0.1][tier],
        };
        if self.ambiguous {
            confidence.min(AMBIGUOUS_CONFIDENCE)
        } else {
            confidence
        }
    }

    /// More than [`WIDE_DISPATCH`] implementations.
    pub fn is_wide(&self) -> bool {
        self.implementations > WIDE_DISPATCH
    }

    /// The edge metadata:
    /// `overrides:<abstract|concrete>;impls:<n>;confidence:<c>`, then
    /// `;abstract-override`, `;universal` and `;ambiguous-overload` where
    /// they apply.
    pub fn to_metadata(&self) -> String {
        let kind = match self.overridden {
            OverriddenKind::Abstract => "abstract",
            OverriddenKind::Concrete => "concrete",
        };
        let mut out = format!(
            "overrides:{kind};impls:{};confidence:{:.1}",
            self.implementations,
            self.confidence()
        );
        if self.abstract_override {
            out.push_str(";abstract-override");
        }
        if self.universal {
            out.push_str(";universal");
        }
        if self.ambiguous {
            out.push_str(";ambiguous-overload");
        }
        out
    }

    /// Reads [`to_metadata`](Self::to_metadata) back; `None` when the
    /// metadata is not an OVERRIDES record.
    pub fn from_metadata(metadata: &str) -> Option<Self> {
        let mut overridden = None;
        let mut implementations = 0;
        let mut abstract_override = false;
        let mut universal = false;
        let mut ambiguous = false;
        for part in metadata.split(';') {
            match part.split_once(':') {
                Some(("overrides", "abstract")) => overridden = Some(OverriddenKind::Abstract),
                Some(("overrides", "concrete")) => overridden = Some(OverriddenKind::Concrete),
                Some(("impls", n)) => implementations = n.parse().unwrap_or(0),
                _ if part == "abstract-override" => abstract_override = true,
                _ if part == "universal" => universal = true,
                _ if part == "ambiguous-overload" => ambiguous = true,
                _ => {}
            }
        }
        Some(Self {
            overridden: overridden?,
            implementations,
            abstract_override,
            universal,
            ambiguous,
        })
    }
}

/// Names of the methods every object has, per language family: their
/// overrides are recorded but never followed.
pub fn is_universal_method(family: &str, name: &str) -> bool {
    match family {
        "jvm" => matches!(
            name,
            "toString" | "equals" | "hashCode" | "clone" | "finalize"
        ),
        "csharp" => matches!(name, "ToString" | "Equals" | "GetHashCode" | "Finalize"),
        "js" => matches!(name, "toString" | "valueOf" | "toJSON"),
        "python" => matches!(
            name,
            "__str__" | "__repr__" | "__eq__" | "__ne__" | "__hash__" | "__init__" | "__new__"
        ),
        _ => false,
    }
}

/// One OVERRIDES edge seen from either end.
#[derive(Debug, Clone, Copy)]
pub struct Override<'a> {
    /// The uid at the other end: the overridden method seen from the
    /// implementation, the implementation seen from the overridden method.
    pub other: &'a str,
    /// The OVERRIDES edge.
    pub edge: &'a Relationship,
    /// What it records.
    pub confidence: DispatchConfidence,
}

/// The OVERRIDES edges of an index, both ways, without the universal ones.
#[derive(Debug, Default)]
pub struct DispatchIndex<'a> {
    /// Implementation uid → the methods it overrides.
    up: HashMap<&'a str, Vec<Override<'a>>>,
    /// Overridden method uid → its concrete implementations, by uid.
    down: HashMap<&'a str, Vec<Override<'a>>>,
}

impl<'a> DispatchIndex<'a> {
    /// Indexes the OVERRIDES edges among `relationships`.
    pub fn new(relationships: &'a [Relationship]) -> Self {
        let mut index = Self::default();
        for edge in relationships
            .iter()
            .filter(|r| r.kind == RelationshipKind::Overrides)
        {
            let Some(confidence) = DispatchConfidence::from_metadata(&edge.metadata) else {
                continue;
            };
            if confidence.universal {
                continue;
            }
            index
                .up
                .entry(edge.source_uid.as_str())
                .or_default()
                .push(Override {
                    other: &edge.target_uid,
                    edge,
                    confidence,
                });
            if !confidence.abstract_override {
                index
                    .down
                    .entry(edge.target_uid.as_str())
                    .or_default()
                    .push(Override {
                        other: &edge.source_uid,
                        edge,
                        confidence,
                    });
            }
        }
        for list in index.up.values_mut().chain(index.down.values_mut()) {
            list.sort_by(|a, b| a.other.cmp(b.other));
        }
        index
    }

    /// Whether the index has no OVERRIDES edge to follow.
    pub fn is_empty(&self) -> bool {
        self.up.is_empty()
    }

    /// The methods `uid` overrides or implements.
    pub fn overridden(&self, uid: &str) -> &[Override<'a>] {
        self.up.get(uid).map_or(&[], Vec::as_slice)
    }

    /// The concrete implementations of `uid`, ordered by uid (callers order
    /// them by location).
    pub fn implementations(&self, uid: &str) -> &[Override<'a>] {
        self.down.get(uid).map_or(&[], Vec::as_slice)
    }

    /// Whether `call` dispatches: a CALLS edge that is not an `ambiguous`
    /// guess ([`dispatches_from`]) and does not come from an override of its
    /// own target. That is `super.m()` from `m`, which binds statically.
    pub fn dispatches(&self, call: &Relationship) -> bool {
        dispatches_from(call)
            && !self
                .overridden(&call.source_uid)
                .iter()
                .any(|o| o.other == call.target_uid)
    }
}

/// How a walk reached a symbol through dynamic dispatch rather than a
/// direct CALLS edge.
#[derive(Debug, Clone, Copy)]
pub struct Dispatch<'a> {
    /// The overridden method the CALLS edge binds to.
    pub base: &'a CodeSymbol,
    /// The OVERRIDES edge between the implementation and `base`.
    pub edge: &'a Relationship,
    /// How likely the call runs the implementation.
    pub confidence: f32,
}

impl Dispatch<'_> {
    /// `dispatch via Base.m, confidence 0.8`, for tool output.
    pub fn describe(&self) -> String {
        format!("dispatch {}", self.provenance())
    }

    /// `via Base.m, confidence 0.8`: where the dispatch came from, apart
    /// from the `dispatch` status label.
    pub fn provenance(&self) -> String {
        format!(
            "via {}, confidence {:.1}",
            self.base.qualified_name, self.confidence
        )
    }
}

/// Where a callee list left implementations out: a call to `base`, which
/// has `total` implementations, of which [`MAX_LISTED_IMPLEMENTATIONS`]
/// are listed.
#[derive(Debug, Clone, Copy)]
pub struct DispatchCut<'a> {
    /// The overridden method called.
    pub base: &'a CodeSymbol,
    /// Its implementations.
    pub total: usize,
    /// How many of them the walk lists.
    pub listed: usize,
}

/// Whether a CALLS edge is certain enough to dispatch from: not an
/// `ambiguous` guess. Such an edge links namesakes (often the interface
/// method and an implementation already) or overloads the argument count
/// does not tell apart; following the implementations of each would add
/// those of the overload not called (on spring-framework, 20 wrong targets
/// for 7 right ones on the dev set).
pub fn dispatches_from(call: &Relationship) -> bool {
    call.kind == RelationshipKind::Calls
        && crate::CallResolution::from_metadata(&call.metadata)
            != Some(crate::CallResolution::Ambiguous)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confidence_falls_with_the_number_of_implementations() {
        let c = |overridden, implementations| DispatchConfidence {
            overridden,
            implementations,
            abstract_override: false,
            universal: false,
            ambiguous: false,
        };
        let abs: Vec<f32> = [1, 3, 8, 16, 17]
            .iter()
            .map(|&n| c(OverriddenKind::Abstract, n).confidence())
            .collect();
        assert_eq!(abs, [0.9, 0.8, 0.6, 0.4, 0.2]);
        assert!(
            c(OverriddenKind::Concrete, 1).confidence()
                < c(OverriddenKind::Abstract, 1).confidence()
        );
        assert!(c(OverriddenKind::Abstract, 17).is_wide());
        assert!(!c(OverriddenKind::Abstract, 16).is_wide());
    }

    #[test]
    fn metadata_round_trips() {
        let record = DispatchConfidence {
            overridden: OverriddenKind::Concrete,
            implementations: 4,
            abstract_override: true,
            universal: true,
            ambiguous: true,
        };
        let meta = record.to_metadata();
        assert_eq!(
            meta,
            "overrides:concrete;impls:4;confidence:0.1;abstract-override;universal;ambiguous-overload"
        );
        let ambiguous = DispatchConfidence {
            overridden: OverriddenKind::Abstract,
            implementations: 1,
            abstract_override: false,
            universal: false,
            ambiguous: true,
        };
        assert_eq!(ambiguous.confidence(), AMBIGUOUS_CONFIDENCE);
        assert_eq!(DispatchConfidence::from_metadata(&meta), Some(record));
        assert_eq!(
            DispatchConfidence::from_metadata("line:3;resolved:exact"),
            None
        );
    }

    #[test]
    fn universal_methods_are_per_family() {
        assert!(is_universal_method("jvm", "toString"));
        assert!(is_universal_method("csharp", "GetHashCode"));
        assert!(!is_universal_method("jvm", "ToString"));
        assert!(!is_universal_method("jvm", "handle"));
    }
}

/// The analyzer end to end on small projects in Java, Kotlin, C# and
/// TypeScript: an interface, an abstract class that re-declares its method,
/// two implementations, a caller through the interface.
#[cfg(test)]
mod fixture_tests {
    use crate::navigation::{walk_calls_reached, CallDirection, Reached};
    use crate::Analyzer;
    use myceliums_storage::{CodeSymbol, Relationship, RelationshipKind, Store};
    use std::path::Path;

    struct Graph {
        symbols: Vec<CodeSymbol>,
        relationships: Vec<Relationship>,
    }

    impl Graph {
        async fn of(files: &[(&str, &str)]) -> Graph {
            let dir = tempfile::TempDir::new().unwrap();
            let root = dir.path().join("project");
            for (path, text) in files {
                let path = root.join(path);
                std::fs::create_dir_all(path.parent().unwrap_or(Path::new("."))).unwrap();
                std::fs::write(path, text).unwrap();
            }
            let store = Store::open(&dir.path().join("db"), "dispatch")
                .await
                .unwrap();
            let analyzer = Analyzer::new(store, root).set_skip_embeddings(true);
            analyzer.analyze().await.unwrap();
            Graph {
                symbols: analyzer.store().get_symbols().await.unwrap(),
                relationships: analyzer.store().get_relationships().await.unwrap(),
            }
        }

        fn symbol(&self, qualified: &str) -> &CodeSymbol {
            let found: Vec<&CodeSymbol> = self
                .symbols
                .iter()
                .filter(|s| s.qualified_name == qualified)
                .collect();
            assert_eq!(found.len(), 1, "one symbol {qualified}: {found:?}");
            found[0]
        }

        fn name(&self, uid: &str) -> &str {
            &self
                .symbols
                .iter()
                .find(|s| s.uid == uid)
                .expect("edge end is a symbol")
                .qualified_name
        }

        /// `source -> target` per edge of `kind`, sorted.
        fn edges(&self, kind: RelationshipKind) -> Vec<String> {
            let mut out: Vec<String> = self
                .relationships
                .iter()
                .filter(|r| r.kind == kind)
                .map(|r| {
                    format!(
                        "{} -> {}",
                        self.name(&r.source_uid),
                        self.name(&r.target_uid)
                    )
                })
                .collect();
            out.sort();
            out
        }

        /// `source:line -> target:line (confidence)` per OVERRIDES edge,
        /// sorted: tells overloads apart.
        fn overrides_at(&self) -> Vec<String> {
            let at = |uid: &str| {
                let s = self.symbols.iter().find(|s| s.uid == uid).unwrap();
                format!("{}:{}", s.qualified_name, s.start_line)
            };
            let mut out: Vec<String> = self
                .relationships
                .iter()
                .filter(|r| r.kind == RelationshipKind::Overrides)
                .map(|r| {
                    let c = super::DispatchConfidence::from_metadata(&r.metadata).unwrap();
                    format!(
                        "{} -> {} ({:.1})",
                        at(&r.source_uid),
                        at(&r.target_uid),
                        c.confidence()
                    )
                })
                .collect();
            out.sort();
            out
        }

        /// Direct callers or callees of `qualified`: `name` for a CALLS
        /// edge, `name (via Base.m)` for dispatch.
        fn walk(&self, qualified: &str, direction: CallDirection) -> Vec<String> {
            let start = self.symbol(qualified);
            let mut out: Vec<String> =
                walk_calls_reached(&self.symbols, &self.relationships, start, direction, 1)
                    .iter()
                    .map(|r: &Reached| match &r.dispatch {
                        Some(d) => format!(
                            "{} (via {}, {:.1})",
                            r.symbol.qualified_name, d.base.qualified_name, d.confidence
                        ),
                        None => r.symbol.qualified_name.clone(),
                    })
                    .collect();
            out.sort();
            out
        }
    }

    #[tokio::test]
    async fn java_implementations_inherit_the_callers_of_the_interface_method() {
        let g = Graph::of(&[
            (
                "shapes/Shape.java",
                "package shapes;\n\npublic interface Shape {\n    double area();\n    String name();\n}\n",
            ),
            (
                "shapes/Polygon.java",
                "package shapes;\n\npublic abstract class Polygon implements Shape {\n    public abstract double area();\n\n    public String name() {\n        return \"polygon\";\n    }\n\n    public String toString() {\n        return name();\n    }\n}\n",
            ),
            (
                "shapes/Square.java",
                "package shapes;\n\npublic class Square extends Polygon {\n    private final double side;\n\n    public Square(double side) {\n        this.side = side;\n    }\n\n    @Override\n    public double area() {\n        return side * side;\n    }\n\n    @Override\n    public String name() {\n        return \"square \" + super.name();\n    }\n\n    @Override\n    public String toString() {\n        return \"Square\";\n    }\n\n    private static double unit() {\n        return 1;\n    }\n}\n",
            ),
            (
                "shapes/Circle.java",
                "package shapes;\n\npublic class Circle implements Shape {\n    public double area() {\n        return 3.14;\n    }\n\n    public String name() {\n        return \"circle\";\n    }\n}\n",
            ),
            (
                "shapes/Report.java",
                "package shapes;\n\npublic class Report {\n    public double total(Shape shape) {\n        return shape.area();\n    }\n\n    public String label(Polygon polygon) {\n        return polygon.toString();\n    }\n}\n",
            ),
        ])
        .await;
        assert_eq!(
            g.edges(RelationshipKind::Overrides),
            [
                "Circle.area -> Shape.area",
                "Circle.name -> Shape.name",
                "Polygon.area -> Shape.area",
                "Polygon.name -> Shape.name",
                "Square.area -> Polygon.area",
                "Square.area -> Shape.area",
                "Square.name -> Polygon.name",
                "Square.name -> Shape.name",
                "Square.toString -> Polygon.toString",
            ]
        );
        // CALLS edges are unchanged: the call binds the interface method.
        assert!(g
            .edges(RelationshipKind::Calls)
            .contains(&"Report.total -> Shape.area".to_string()));
        // Two implementations of an interface method: confidence 0.8.
        assert_eq!(
            g.walk("Square.area", CallDirection::Callers),
            ["Report.total (via Shape.area, 0.8)"]
        );
        assert_eq!(
            g.walk("Circle.area", CallDirection::Callers),
            ["Report.total (via Shape.area, 0.8)"]
        );
        // The abstract re-declaration never runs: not listed as a callee.
        assert_eq!(
            g.walk("Report.total", CallDirection::Callees),
            [
                "Circle.area (via Shape.area, 0.8)",
                "Shape.area",
                "Square.area (via Shape.area, 0.8)"
            ]
        );
        // `super.name()` binds Polygon.name statically: no dispatch to the
        // caller itself or to Circle.name.
        assert_eq!(
            g.walk("Square.name", CallDirection::Callees),
            ["Polygon.name"]
        );
        // toString is universal: recorded, never followed.
        assert!(g
            .relationships
            .iter()
            .any(|r| r.kind == RelationshipKind::Overrides && r.metadata.ends_with(";universal")));
        assert_eq!(
            g.walk("Square.toString", CallDirection::Callers),
            Vec::<String>::new()
        );
    }

    #[tokio::test]
    async fn kotlin_implementations_inherit_the_callers_of_the_interface_method() {
        let g = Graph::of(&[
            (
                "shapes/Shape.kt",
                "package shapes\n\ninterface Shape {\n    fun area(): Double\n}\n\nabstract class Polygon : Shape {\n    abstract override fun area(): Double\n}\n",
            ),
            (
                "shapes/Square.kt",
                "package shapes\n\nclass Square(val side: Double) : Polygon() {\n    override fun area(): Double {\n        return side * side\n    }\n}\n\nclass Circle : Shape {\n    override fun area(): Double = 3.14\n}\n",
            ),
            (
                "shapes/Report.kt",
                "package shapes\n\nclass Report {\n    fun total(shape: Shape): Double {\n        return shape.area()\n    }\n}\n",
            ),
        ])
        .await;
        assert_eq!(
            g.edges(RelationshipKind::Overrides),
            [
                "Circle.area -> Shape.area",
                "Polygon.area -> Shape.area",
                "Square.area -> Polygon.area",
                "Square.area -> Shape.area",
            ]
        );
        // Matched by name, the call binds the method the others override.
        assert_eq!(
            g.walk("Report.total", CallDirection::Callees),
            [
                "Circle.area (via Shape.area, 0.8)",
                "Shape.area",
                "Square.area (via Shape.area, 0.8)"
            ]
        );
        assert_eq!(
            g.walk("Square.area", CallDirection::Callers),
            ["Report.total (via Shape.area, 0.8)"]
        );
    }

    #[tokio::test]
    async fn csharp_implementations_inherit_the_callers_of_the_interface_method() {
        let g = Graph::of(&[
            (
                "Shapes/IShape.cs",
                "namespace Shapes\n{\n    public interface IShape\n    {\n        double Area();\n    }\n}\n",
            ),
            (
                "Shapes/Polygon.cs",
                "namespace Shapes\n{\n    public abstract class Polygon : IShape\n    {\n        public abstract double Area();\n    }\n\n    public class Square : Polygon\n    {\n        public override double Area()\n        {\n            return 4;\n        }\n    }\n}\n",
            ),
            (
                "Shapes/Circle.cs",
                "namespace Shapes\n{\n    public class Circle : IShape\n    {\n        public double Area()\n        {\n            return 3.14;\n        }\n    }\n}\n",
            ),
            (
                "Shapes/Report.cs",
                "namespace Shapes\n{\n    public class Report\n    {\n        public double Total(IShape shape)\n        {\n            return shape.Area();\n        }\n    }\n}\n",
            ),
        ])
        .await;
        assert_eq!(
            g.edges(RelationshipKind::Overrides),
            [
                "Circle.Area -> IShape.Area",
                "Polygon.Area -> IShape.Area",
                "Square.Area -> IShape.Area",
                "Square.Area -> Polygon.Area",
            ]
        );
        assert_eq!(
            g.walk("Report.Total", CallDirection::Callees),
            [
                "Circle.Area (via IShape.Area, 0.8)",
                "IShape.Area",
                "Square.Area (via IShape.Area, 0.8)"
            ]
        );
        assert_eq!(
            g.walk("Circle.Area", CallDirection::Callers),
            ["Report.Total (via IShape.Area, 0.8)"]
        );
    }

    #[tokio::test]
    async fn typescript_implementations_inherit_the_callers_of_the_interface_method() {
        let g = Graph::of(&[
            (
                "src/shapes.ts",
                "export interface Shape {\n  area(): number;\n}\n\nexport abstract class Polygon implements Shape {\n  abstract area(): number;\n}\n\nexport class Square extends Polygon {\n  constructor(private side: number) {\n    super();\n  }\n\n  area(): number {\n    return this.side * this.side;\n  }\n}\n\nexport class Circle implements Shape {\n  area(): number {\n    return 3.14;\n  }\n}\n",
            ),
            (
                "src/report.ts",
                "import { Shape } from \"./shapes\";\n\nexport function total(shape: Shape): number {\n  return shape.area();\n}\n",
            ),
        ])
        .await;
        assert_eq!(
            g.edges(RelationshipKind::Overrides),
            [
                "Circle.area -> Shape.area",
                "Polygon.area -> Shape.area",
                "Square.area -> Polygon.area",
                "Square.area -> Shape.area",
            ]
        );
        assert_eq!(
            g.walk("total", CallDirection::Callees),
            [
                "Circle.area (via Shape.area, 0.8)",
                "Shape.area",
                "Square.area (via Shape.area, 0.8)"
            ]
        );
        assert_eq!(
            g.walk("Square.area", CallDirection::Callers),
            ["total (via Shape.area, 0.8)"]
        );
    }

    /// Java: an implementation overrides the overload whose parameter types
    /// it shares; a type parameter matches any type; default methods are
    /// overridable; final, static and private methods are not.
    #[tokio::test]
    async fn java_overrides_respect_overloads_generics_and_modifiers() {
        let g = Graph::of(&[
            (
                "p/Handler.java",
                "package p;\n\npublic interface Handler<T> {\n    void handle(String event);\n\n    void handle(Integer event);\n\n    T convert(T input);\n\n    default String describe() {\n        return \"handler\";\n    }\n\n    default void log() {\n    }\n}\n",
            ),
            (
                "p/Base.java",
                "package p;\n\npublic class Base {\n    public final void fixedStep() {\n    }\n\n    public static void util() {\n    }\n\n    private void secret() {\n    }\n\n    public void open() {\n    }\n}\n",
            ),
            (
                "p/Impl.java",
                "package p;\n\npublic class Impl extends Base implements Handler<String> {\n    public void handle(String event) {\n    }\n\n    public String convert(String input) {\n        return input;\n    }\n\n    public String describe() {\n        return \"impl\";\n    }\n\n    public void fixedStep() {\n    }\n\n    public static void util() {\n    }\n\n    private void secret() {\n    }\n\n    public void open() {\n    }\n}\n",
            ),
            (
                "p/Relay.java",
                "package p;\n\npublic class Relay<X> implements Handler<X> {\n    public void handle(X event) {\n    }\n}\n",
            ),
        ])
        .await;
        assert_eq!(
            g.overrides_at(),
            [
                // The default method has a body: a concrete method.
                "Impl.convert:7 -> Handler.convert:8 (0.9)",
                "Impl.describe:11 -> Handler.describe:10 (0.7)",
                // handle(String), not handle(Integer).
                "Impl.handle:4 -> Handler.handle:4 (0.8)",
                "Impl.open:24 -> Base.open:13 (0.7)",
                // handle(X) fits both overloads: an edge to each, ambiguous.
                "Relay.handle:4 -> Handler.handle:4 (0.2)",
                "Relay.handle:4 -> Handler.handle:6 (0.2)",
            ]
        );
    }

    /// C#: only `virtual`/`abstract`/`override` methods are overridden,
    /// only by `override`; `new` hides. Kotlin: only `open` methods, only
    /// by `override`.
    #[tokio::test]
    async fn csharp_and_kotlin_overrides_follow_their_modifiers() {
        let g = Graph::of(&[
            (
                "Zoo/Animal.cs",
                "namespace Zoo\n{\n    public class Animal\n    {\n        public virtual string Speak() { return \"\"; }\n\n        public string Name() { return \"\"; }\n\n        public virtual void Eat() { }\n    }\n\n    public class Dog : Animal\n    {\n        public override string Speak() { return \"woof\"; }\n\n        public new string Name() { return \"dog\"; }\n\n        public new void Eat() { }\n    }\n}\n",
            ),
            (
                "zoo/Base.kt",
                "package zoo\n\nopen class Keeper {\n    fun closed() {\n    }\n\n    open fun opened() {\n    }\n}\n\nclass Junior : Keeper() {\n    fun closed() {\n    }\n\n    override fun opened() {\n    }\n}\n",
            ),
        ])
        .await;
        assert_eq!(
            g.overrides_at(),
            [
                "Dog.Speak:14 -> Animal.Speak:5 (0.7)",
                "Junior.opened:15 -> Keeper.opened:7 (0.7)",
            ]
        );
    }

    /// Rust: a trait's method is implemented by `impl Trait for Type`, not
    /// by a namesake in another type's inherent impl. TypeScript: a class
    /// implements an interface only when it says so, not by its shape.
    #[tokio::test]
    async fn rust_traits_and_typescript_shapes_do_not_over_link() {
        let g = Graph::of(&[
            (
                "src/lib.rs",
                "pub trait Shape {\n    fn area(&self) -> f64;\n}\n\npub struct Square;\n\nimpl Shape for Square {\n    fn area(&self) -> f64 {\n        1.0\n    }\n}\n\npub struct Plot;\n\nimpl Plot {\n    pub fn area(&self) -> f64 {\n        2.0\n    }\n}\n",
            ),
            (
                "web/shapes.ts",
                "export interface Figure {\n  area(): number;\n}\n\nexport class Tile implements Figure {\n  area(): number {\n    return 1;\n  }\n}\n\nexport class Lookalike {\n  area(): number {\n    return 2;\n  }\n}\n\nexport function total(figure: Figure): number {\n  return figure.area();\n}\n",
            ),
        ])
        .await;
        assert_eq!(
            g.overrides_at(),
            [
                "Square.area:8 -> Shape.area:2 (0.9)",
                "Tile.area:6 -> Figure.area:2 (0.9)",
            ]
        );
        assert_eq!(
            g.walk("total", CallDirection::Callees),
            ["Figure.area", "Tile.area (via Figure.area, 0.9)"]
        );
    }
}
