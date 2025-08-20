// benches/fibonacci_performance.rs
use criterion::{criterion_group, criterion_main, black_box, Criterion, BenchmarkId};

fn fibonacci_recursive(n: u64) -> u64 {
    match n {
        0 => 1,
        1 => 1,
        n => fibonacci_recursive(n-1) + fibonacci_recursive(n-2),
    }
}

fn fibonacci_iterative(n: u64) -> u64 {
    if n <= 1 { return 1; }
    let mut a = 1;
    let mut b = 1;
    for _ in 2..=n {
        let temp = a + b;
        a = b;
        b = temp;
    }
    b
}

fn fibonacci_memoized(n: u64, cache: &mut std::collections::HashMap<u64, u64>) -> u64 {
    if let Some(&result) = cache.get(&n) {
        return result;
    }
    
    let result = match n {
        0 => 1,
        1 => 1,
        n => fibonacci_memoized(n-1, cache) + fibonacci_memoized(n-2, cache),
    };
    
    cache.insert(n, result);
    result
}

// CPU Performance Benchmarks
fn cpu_performance(c: &mut Criterion) {
    let mut group = c.benchmark_group("fibonacci_cpu");
    
    // Small inputs for recursive (to avoid timeout)
    for &n in &[10, 15, 20] {
        group.bench_with_input(
            BenchmarkId::new("recursive", n),
            &n,
            |b, &n| b.iter(|| fibonacci_recursive(black_box(n)))
        );
    }
    
    // Medium inputs for iterative and memoized
    for &n in &[20, 30, 40] {
        group.bench_with_input(
            BenchmarkId::new("iterative", n),
            &n,
            |b, &n| b.iter(|| fibonacci_iterative(black_box(n)))
        );
        
        group.bench_with_input(
            BenchmarkId::new("memoized", n),
            &n,
            |b, &n| {
                b.iter(|| {
                    let mut cache = std::collections::HashMap::new();
                    fibonacci_memoized(black_box(n), &mut cache)
                })
            }
        );
    }
    
    group.finish();
}

// Memory efficiency tests
fn memory_efficiency(c: &mut Criterion) {
    let mut group = c.benchmark_group("fibonacci_memory");
    
    // Test constant memory usage (iterative)
    group.bench_function("iterative_fib_50", |b| {
        b.iter(|| fibonacci_iterative(black_box(50)))
    });
    
    group.bench_function("iterative_fib_100", |b| {
        b.iter(|| fibonacci_iterative(black_box(100)))
    });
    
    // Test heap allocation (memoized)
    group.bench_function("memoized_fib_50", |b| {
        b.iter(|| {
            let mut cache = std::collections::HashMap::new();
            fibonacci_memoized(black_box(50), &mut cache)
        })
    });
    
    group.finish();
}

// Scalability tests
fn scalability(c: &mut Criterion) {
    let mut group = c.benchmark_group("fibonacci_scale");
    
    // Test how performance scales with input size
    for &n in &[10, 20, 30, 40, 50] {
        group.bench_with_input(
            BenchmarkId::new("iterative_scale", n),
            &n,
            |b, &n| b.iter(|| fibonacci_iterative(black_box(n)))
        );
    }
    
    group.finish();
}

// Regression tests - track specific cases over time
fn regression_tracking(c: &mut Criterion) {
    // These specific benchmarks will be tracked for performance regressions
    c.bench_function("regression_recursive_fib_20", |b| {
        b.iter(|| fibonacci_recursive(black_box(20)))
    });
    
    c.bench_function("regression_iterative_fib_50", |b| {
        b.iter(|| fibonacci_iterative(black_box(50)))
    });
    
    c.bench_function("regression_memoized_fib_30", |b| {
        b.iter(|| {
            let mut cache = std::collections::HashMap::new();
            fibonacci_memoized(black_box(30), &mut cache)
        })
    });
}

criterion_group!(
    benches,
    cpu_performance,
    memory_efficiency, 
    scalability,
    regression_tracking
);
criterion_main!(benches);