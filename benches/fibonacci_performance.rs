// use std::time::Instant;
// use std::process;

// fn fibonacci_recursive(n: u64) -> u64 {
//     match n {
//         0 => 1,
//         1 => 1,
//         n => fibonacci_recursive(n-1) + fibonacci_recursive(n-2),
//     }
// }

// fn fibonacci_iterative(n: u64) -> u64 {
//     if n <= 1 { return 1; }
//     let mut a = 1;
//     let mut b = 1;
//     for _ in 2..=n {
//         let temp = a + b;
//         a = b;
//         b = temp;
//     }
//     b
// }

// fn measure_memory_usage() -> usize {
//     // Get current memory usage (works on Unix systems)
//     let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
//     for line in status.lines() {
//         if line.starts_with("VmRSS:") {
//             if let Some(kb_str) = line.split_whitespace().nth(1) {
//                 return kb_str.parse::<usize>().unwrap_or(0) * 1024; // Convert KB to bytes
//             }
//         }
//     }
//     0
// }

// fn benchmark_function<F>(name: &str, f: F, input: u64, iterations: u32) 
// where 
//     F: Fn(u64) -> u64,
// {
//     println!("\n=== Benchmarking {} with input {} ===", name, input);
    
//     // Warmup
//     for _ in 0..5 {
//         let _ = f(input);
//     }
    
//     let mem_before = measure_memory_usage();
//     let start = Instant::now();
    
//     let mut results = Vec::new();
//     for _ in 0..iterations {
//         let iter_start = Instant::now();
//         let result = f(input);
//         let iter_duration = iter_start.elapsed();
//         results.push((result, iter_duration));
//     }
    
//     let total_duration = start.elapsed();
//     let mem_after = measure_memory_usage();
    
//     // Calculate statistics
//     let total_nanos: u128 = results.iter().map(|(_, d)| d.as_nanos()).sum();
//     let avg_nanos = total_nanos / iterations as u128;
//     let min_nanos = results.iter().map(|(_, d)| d.as_nanos()).min().unwrap();
//     let max_nanos = results.iter().map(|(_, d)| d.as_nanos()).max().unwrap();
    
//     println!("Result: {}", results[0].0);
//     println!("Iterations: {}", iterations);
//     println!("Total time: {:?}", total_duration);
//     println!("Average time: {:.2} μs", avg_nanos as f64 / 1000.0);
//     println!("Min time: {:.2} μs", min_nanos as f64 / 1000.0);
//     println!("Max time: {:.2} μs", max_nanos as f64 / 1000.0);
//     println!("Memory before: {} bytes", mem_before);
//     println!("Memory after: {} bytes", mem_after);
//     println!("Memory delta: {} bytes", mem_after as i64 - mem_before as i64);
    
//     // Calculate operations per second
//     let ops_per_sec = (iterations as f64) / total_duration.as_secs_f64();
//     println!("Operations/second: {:.2}", ops_per_sec);
// }

// fn cpu_intensive_test() {
//     println!("\n=== CPU Usage Test ===");
    
//     let start = Instant::now();
//     let mut cpu_start = process::id();
    
//     // Run recursive fibonacci multiple times to stress CPU
//     let mut total = 0u64;
//     for i in 1..=25 {
//         total = total.wrapping_add(fibonacci_recursive(i));
//     }
    
//     let duration = start.elapsed();
//     println!("CPU intensive computation completed");
//     println!("Total time: {:?}", duration);
//     println!("Result checksum: {}", total);
//     println!("CPU utilization: High (recursive calls create many stack frames)");
// }

// fn memory_stress_test() {
//     println!("\n=== Memory Usage Test ===");
    
//     let mem_start = measure_memory_usage();
    
//     // Test stack usage with deep recursion
//     let start = Instant::now();
//     let result = fibonacci_recursive(30); // This will use significant stack
//     let duration = start.elapsed();
    
//     let mem_end = measure_memory_usage();
    
//     println!("Deep recursion (fib 30): {}", result);
//     println!("Time: {:?}", duration);
//     println!("Memory start: {} bytes", mem_start);
//     println!("Memory end: {} bytes", mem_end);
//     println!("Stack frames created: ~{}", 2_u64.pow(30) / 1000000); // Approximate
    
//     // Compare with iterative (constant memory)
//     let mem_start2 = measure_memory_usage();
//     let start2 = Instant::now();
//     let result2 = fibonacci_iterative(100); // Much larger input
//     let duration2 = start2.elapsed();
//     let mem_end2 = measure_memory_usage();
    
//     println!("\nIterative (fib 100): {}", result2);
//     println!("Time: {:?}", duration2);
//     println!("Memory delta: {} bytes", mem_end2 as i64 - mem_start2 as i64);
//     println!("Memory efficiency: Constant (O(1))");
// }

// fn main() {
//     println!("🚀 FIBONACCI PERFORMANCE ANALYSIS 🚀");
    
//     // Test small inputs
//     benchmark_function("Recursive", fibonacci_recursive, 20, 100);
//     benchmark_function("Iterative", fibonacci_iterative, 20, 100);
    
//     println!("\n{}", "=".repeat(50));
    
//     // Test medium inputs (only iterative, recursive would be too slow)
//     benchmark_function("Iterative", fibonacci_iterative, 50, 1000);
    
//     println!("\n{}", "=".repeat(50));
    
//     // Test large inputs
//     benchmark_function("Iterative", fibonacci_iterative, 100, 1000);
    
//     println!("\n{}", "=".repeat(50));
    
//     // CPU and memory tests
//     cpu_intensive_test();
//     memory_stress_test();
    
//     println!("\n🎯 PERFORMANCE SUMMARY:");
//     println!("• Recursive: O(2^n) time, O(n) space - EXPONENTIALLY SLOW");
//     println!("• Iterative: O(n) time, O(1) space - LINEAR AND EFFICIENT");
//     println!("• For n=30: Recursive ~1,073,741,824 operations vs Iterative ~30 operations");
// }