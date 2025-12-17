//! Performance benchmarks - native only (uses filesystem APIs)
#![cfg(not(target_arch = "wasm32"))]

use transientdb;

use serde_json::json;
use std::io::Result;
use std::time::{Duration, Instant};
use tempfile::TempDir;
use transientdb::TransientDB;
use transientdb::{DirectoryConfig, DirectoryStore};
use transientdb::{MemoryConfig, MemoryStore};

/* run these tests locally with:

cargo test --release benchmark_memory_store -- --nocapture
cargo test --release benchmark_directory_store -- --nocapture

 */

const NUM_EVENTS: usize = 1_000_000;
const REPORT_INTERVAL: usize = 100_000;

// Simple latency tracker
struct LatencyStats {
	p50: Duration,
	p95: Duration,
	p99: Duration,
	samples: Vec<Duration>,
}

impl LatencyStats {
	fn new() -> Self {
		Self {
			p50: Duration::default(),
			p95: Duration::default(),
			p99: Duration::default(),
			samples: Vec::new(),
		}
	}

	fn add(&mut self, d: Duration) {
		self.samples.push(d);
	}

	fn calculate(&mut self) {
		if self.samples.is_empty() {
			return;
		}
		self.samples.sort();
		let len = self.samples.len();
		self.p50 = self.samples[len * 50 / 100];
		self.p95 = self.samples[len * 95 / 100];
		self.p99 = self.samples[len * 99 / 100];
	}
}

#[test]
fn benchmark_memory_store() -> Result<()> {
	println!("\n=== Memory Store Performance Test ===");

	let config = MemoryConfig {
		write_key: "bench-key".to_string(),
		max_items: 2_000_000,             // 2M to avoid FIFO cleanup during test
		max_fetch_size: 1024 * 1024 * 10, // 10MB
	};

	let store = MemoryStore::new(config);
	let db = TransientDB::new(store);

	// Write test
	println!("\nWrite Test - {} events", NUM_EVENTS);
	let mut write_latencies = LatencyStats::new();
	let write_start = Instant::now();

	for i in 0..NUM_EVENTS {
		let event = json!({
			"id": i,
			"timestamp": chrono::Utc::now().to_rfc3339(),
			"data": "sample payload that's reasonably sized",
			"nested": {
				"field1": "value1",
				"field2": 12345
			}
		});

		let op_start = Instant::now();
		db.append(event)?;
		write_latencies.add(op_start.elapsed());

		if (i + 1) % REPORT_INTERVAL == 0 {
			println!("Wrote {}k events", (i + 1) / 1000);
		}
	}

	let write_duration = write_start.elapsed();
	write_latencies.calculate();

	println!("\nWrite Results:");
	println!("Total time: {:?}", write_duration);
	println!(
		"Events per second: {:.2}",
		NUM_EVENTS as f64 / write_duration.as_secs_f64()
	);
	println!("P50 latency: {:?}", write_latencies.p50);
	println!("P95 latency: {:?}", write_latencies.p95);
	println!("P99 latency: {:?}", write_latencies.p99);

	// Read test
	println!("\nRead Test with different batch sizes");
	let batch_sizes = [1000, 5000, 10000];

	for &batch_size in &batch_sizes {
		let mut read_latencies = LatencyStats::new();
		let read_start = Instant::now();
		let mut total_events = 0;

		// Read for 5 seconds or until no more data
		println!("\nBatch size: {}", batch_size);
		while read_start.elapsed() < Duration::from_secs(5) {
			let op_start = Instant::now();
			if let Ok(Some(result)) = db.fetch(Some(batch_size), None) {
				let batch: serde_json::Value = result.data.unwrap();
				let items = batch["batch"].as_array().unwrap();
				total_events += items.len();
				read_latencies.add(op_start.elapsed());
			} else {
				break;
			}
		}

		let read_duration = read_start.elapsed();
		read_latencies.calculate();

		println!("Events read: {}", total_events);
		println!("Total time: {:?}", read_duration);
		println!(
			"Events per second: {:.2}",
			total_events as f64 / read_duration.as_secs_f64()
		);
		println!("P50 latency: {:?}", read_latencies.p50);
		println!("P95 latency: {:?}", read_latencies.p95);
		println!("P99 latency: {:?}", read_latencies.p99);
	}

	Ok(())
}

#[test]
fn benchmark_directory_store() -> Result<()> {
	println!("\n=== Directory Store Performance Test ===");

	let temp_dir = TempDir::new()?;
	let config = DirectoryConfig {
		write_key: "bench-key".to_string(),
		storage_location: temp_dir.path().to_owned(),
		base_filename: "events".to_string(),
		max_file_size: 1024 * 1024, // 1MB files
	};

	let store = DirectoryStore::new(config)?;
	let db = TransientDB::new(store);

	// Write test
	println!("\nWrite Test - {} events", NUM_EVENTS);
	let mut write_latencies = LatencyStats::new();
	let write_start = Instant::now();

	for i in 0..NUM_EVENTS {
		let event = json!({
			"id": i,
			"timestamp": chrono::Utc::now().to_rfc3339(),
			"data": "sample payload that's reasonably sized",
			"nested": {
				"field1": "value1",
				"field2": 12345
			}
		});

		let op_start = Instant::now();
		db.append(event)?;
		write_latencies.add(op_start.elapsed());

		if (i + 1) % REPORT_INTERVAL == 0 {
			println!("Wrote {}k events", (i + 1) / 1000);
		}
	}

	let write_duration = write_start.elapsed();
	write_latencies.calculate();

	println!("\nWrite Results:");
	println!("Total time: {:?}", write_duration);
	println!(
		"Events per second: {:.2}",
		NUM_EVENTS as f64 / write_duration.as_secs_f64()
	);
	println!("P50 latency: {:?}", write_latencies.p50);
	println!("P95 latency: {:?}", write_latencies.p95);
	println!("P99 latency: {:?}", write_latencies.p99);

	// File system stats after writes
	let num_files = std::fs::read_dir(&temp_dir)?.count();
	let total_size: u64 = std::fs::read_dir(&temp_dir)?
		.filter_map(Result::ok)
		.filter_map(|entry| entry.metadata().ok())
		.map(|meta| meta.len())
		.sum();

	println!("\nFile System Stats:");
	println!("Number of files: {}", num_files);
	println!(
		"Total size: {:.2} MB",
		total_size as f64 / (1024.0 * 1024.0)
	);
	println!(
		"Average file size: {:.2} KB",
		(total_size as f64 / num_files as f64) / 1024.0
	);

	// Read test
	println!("\nRead Test with different batch sizes");
	let batch_sizes = [5, 10, 20]; // Number of files to fetch at once

	for &batch_size in &batch_sizes {
		let mut read_latencies = LatencyStats::new();
		let read_start = Instant::now();
		let mut total_files = 0;

		// Read for 5 seconds or until no more data
		println!("\nBatch size: {} files", batch_size);
		while read_start.elapsed() < Duration::from_secs(5) {
			let op_start = Instant::now();
			if let Ok(Some(result)) = db.fetch(Some(batch_size), None) {
				if let Some(files) = result.data {
					total_files += files.len();
					read_latencies.add(op_start.elapsed());

					// Clean up files
					if let Some(removable) = result.removable {
						db.remove(&removable)?;
					}
				}
			} else {
				break;
			}
		}

		let read_duration = read_start.elapsed();
		read_latencies.calculate();

		println!("Files processed: {}", total_files);
		println!("Total time: {:?}", read_duration);
		println!(
			"Files per second: {:.2}",
			total_files as f64 / read_duration.as_secs_f64()
		);
		println!("P50 latency: {:?}", read_latencies.p50);
		println!("P95 latency: {:?}", read_latencies.p95);
		println!("P99 latency: {:?}", read_latencies.p99);
	}

	Ok(())
}
