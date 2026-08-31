use cel::context::{Context, VariableResolver};
use cel::parser::Parser;
use cel::{PreparedValue, Program, Value};
use criterion::{black_box, criterion_group, BenchmarkId, Criterion};
use std::collections::HashMap;

const EXPRESSIONS: [(&str, &str); 34] = [
    ("ternary_1", "(false || true) ? 1 : 2"),
    ("ternary_2", "(true ? false : true) ? 1 : 2"),
    ("or_1", "false || true"),
    ("and_1", "true && false"),
    ("and_2", "true && (false ? 2 : 3) > 2"),
    ("number", "1"),
    ("construct_list", "[1,2,3]"),
    ("construct_list_1", "[1]"),
    ("construct_list_2", "[a, 2]"),
    ("add_list", "[1,2,3] + [4, 5, 6]"),
    ("list_element", "[1,2,3][1]"),
    ("construct_dict", "{1: 2, '3': '4'}"),
    ("add_string", "'abc' + 'def'"),
    ("mapexpr", "{1 + a: 3}"),
    ("size_list", "[1].size()"),
    ("size_list_1", "size([1])"),
    ("size_str", "'a'.size()"),
    ("size_str_2", "size('a')"),
    ("size_map", "{1:2}.size()"),
    ("size_map_2", "size({1:2})"),
    ("member", "foo.bar"),
    ("map has", "has(foo.bar.baz)"),
    ("map macro", "[1, 2, 3].map(x, x * 2)"),
    ("filter macro", "[1, 2, 3].filter(x, x > 2)"),
    ("all macro", "[1, 2, 3].all(x, x > 0)"),
    ("all map macro", "{0: 0, 1:1, 2:2}.all(x, x >= 0)"),
    ("max", "max(1, 2, 3)"),
    ("max negative", "max(-1, 0, 1)"),
    ("max float", "max(-1.0, 0.0, 1.0)"),
    ("duration", "duration('1s')"),
    ("timestamp", "timestamp('2023-05-28T00:00:00Z')"), // ("complex", "Account{user_id: 123}.user_id == 123"),
    ("variable resolver", "banana"),
    ("variable hashmap", "apple"),
    ("stress", "true && true && true && true && true && true && true && true && true && true && true && true && true && true && true && true && true && true && true && true && true && true && true && true && true && true && true && true && true && true && true && true && true && true && true && true && true && true && true && true && true && true && true && true && true && true && true && true && true && true && true && true && true && true && true && true && true && true && true && true && true"),
];

struct Resolver;

impl VariableResolver for Resolver {
    fn resolve(&self, expr: &str) -> Option<Value> {
        const V: Value = Value::Bool(false);
        const NOT_V: Value = Value::Bool(true);
        match expr {
            "fruit" => Some(NOT_V),
            "carrot" => Some(NOT_V),
            "orange" => Some(NOT_V),
            "banana" => Some(V),
            _ => None,
        }
    }
}

pub fn criterion_benchmark(c: &mut Criterion) {
    // https://gist.github.com/rhnvrm/db4567fcd87b2cb8e997999e1366d406
    let mut execution_group = c.benchmark_group("execute");
    for (name, expr) in black_box(&EXPRESSIONS) {
        execution_group.bench_function(BenchmarkId::from_parameter(name), |b| {
            let parser = Parser::default();
            let ast = parser.parse(expr).expect("Parsing failed");
            let mut ctx = Context::default();
            ctx.add_variable_from_value("foo", HashMap::from([("bar", 1)]));
            ctx.add_variable_from_value("apple", true);
            ctx.add_variable_from_value("a", 1);
            ctx.set_variable_resolver(&Resolver);
            b.iter(|| Value::resolve_val(&ast, &ctx).expect("Eval failed!"))
        });
    }
}

pub fn criterion_benchmark_parsing(c: &mut Criterion) {
    let mut parsing_group = c.benchmark_group("parse");
    for (name, expr) in black_box(&EXPRESSIONS) {
        parsing_group.bench_function(BenchmarkId::from_parameter(name), |b| {
            b.iter(|| Program::compile(expr).expect("Parsing failed"))
        });
    }
}

fn prepared_fixture(object_count: usize) -> Value {
    let objects = (0..object_count)
        .map(|i| {
            Value::from(HashMap::from([
                ("id", Value::Int(i as i64)),
                ("active", Value::Bool(i % 2 == 0)),
                ("score", Value::Int(i as i64)),
                (
                    "profile",
                    Value::from(HashMap::from([
                        ("enabled", Value::Bool(true)),
                        (
                            "padding",
                            Value::from((0..20).map(Value::Int).collect::<Vec<_>>()),
                        ),
                    ])),
                ),
            ]))
        })
        .collect::<Vec<_>>();
    Value::from(HashMap::from([("objects", Value::from(objects))]))
}

pub fn prepared_context_benchmark(c: &mut Criterion) {
    let expressions = [
        ("nested_bool", "data.objects[3].profile.enabled"),
        (
            "primitive_predicate",
            "data.objects[3].active && data.objects[3].score >= 3",
        ),
    ];
    let mut group = c.benchmark_group("prepared context");

    for object_count in [50, 500, 5_000] {
        let prepared = PreparedValue::try_from_value(prepared_fixture(object_count)).unwrap();
        for (name, expression) in expressions {
            let program = Program::compile(expression).unwrap();
            let mut context = Context::default();
            context.add_prepared_variable("data", prepared.clone());

            group.bench_function(format!("execute_{name}_{object_count}"), |b| {
                b.iter(|| black_box(program.execute(black_box(&context)).unwrap()))
            });
            group.bench_function(format!("replace_and_execute_{name}_{object_count}"), |b| {
                b.iter(|| {
                    context.add_prepared_variable("data", prepared.clone());
                    black_box(program.execute(black_box(&context)).unwrap())
                })
            });
        }
    }
    group.finish();
}

pub fn map_macro_benchmark(c: &mut Criterion) {
    let mut group = c.benchmark_group("map list");
    let sizes = vec![1, 10, 100, 1000, 10000, 100000];

    for size in sizes {
        group.bench_function(format!("map_{size}").as_str(), |b| {
            let list = (0..size).collect::<Vec<_>>();
            let parser = Parser::default();
            let ast = parser.parse("list.map(x, x * 2)").expect("Parsing failed");
            let mut ctx = Context::default();
            ctx.add_variable_from_value("list", list);
            b.iter(|| Value::resolve_val(&ast, &ctx).expect("Eval failed!"))
        });
    }
    group.finish();
}

criterion_group! {
    name = benches;
    config = Criterion::default();
    targets = criterion_benchmark, criterion_benchmark_parsing, prepared_context_benchmark, map_macro_benchmark
}

#[cfg(feature = "dhat-heap")]
#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

/// This is the following macro expanded:
/// criterion_main!(benches);
/// But expanded manually so that we can keep the dhat profiler in scope until after benchmarks run
fn main() {
    #[cfg(feature = "dhat-heap")]
    let profiler = dhat::Profiler::new_heap();

    benches();
    // If adding new criterion groups, do so here.

    // Dropping the dhat profiler prints information to stderr: https://docs.rs/dhat/latest/dhat/
    // Doing so before the below ensures profiler doesn't measure Criterion's summary code.
    // It still may measure other bits of Criterion during the benchmark, of course..
    #[cfg(feature = "dhat-heap")]
    drop(profiler);

    Criterion::default().configure_from_args().final_summary();
}
