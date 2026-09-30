use clouddirstat_core::{format_bytes, format_count, format_usd};
use clouddirstat_providers::s3::{Estimate, S3Location, S3Pricing, list_cost_usd};

/// Prints bucket totals from CloudWatch, and what a full scan would cost.
pub fn print(location: &S3Location, estimate: &Estimate) {
    let as_of = estimate
        .as_of()
        .map_or_else(|| "no data yet".to_owned(), |date| date.to_string());
    println!(
        "{location}  {} in {} objects (CloudWatch, {as_of})",
        format_bytes(estimate.bytes()),
        format_count(estimate.objects())
    );
    if !location.prefix.is_empty() {
        println!("Totals are for the whole bucket; CloudWatch does not report prefixes.");
    }
    println!(
        "Estimated storage cost ~{}/month (list prices from {})",
        format_usd(estimate.monthly_cost()),
        S3Pricing::published()
    );
    println!(
        "A full scan needs about {} LIST requests (~${:.4}); this estimate cost ~${:.4}",
        format_count(estimate.list_requests()),
        estimate.scan_cost_usd(),
        estimate.request_cost_usd()
    );

    let classes = estimate.classes();
    if !classes.is_empty() {
        println!();
        println!("By storage class");
        let total = estimate.bytes().max(1);
        for (class, bytes) in classes {
            println!(
                "  {:<22} {:>10} {:>6.1}%",
                class,
                format_bytes(bytes),
                bytes as f64 * 100.0 / total as f64
            );
        }
    }

    if location.is_all_buckets() {
        println!();
        println!(
            "  {:<32} {:<15} {:>10} {:>14} {:>12} {:>10}  As of",
            "Bucket", "Region", "Size", "Objects", "Cost/mo", "Scan cost"
        );
        for bucket in &estimate.buckets {
            println!(
                "  {:<32} {:<15} {:>10} {:>14} {:>12} {:>10}  {}",
                bucket.bucket,
                bucket.region,
                format_bytes(bucket.bytes()),
                bucket.objects.map_or_else(|| "-".to_owned(), format_count),
                format_usd(bucket.monthly_cost),
                format!("${:.4}", list_cost_usd(bucket.list_requests())),
                bucket
                    .as_of
                    .map_or_else(|| "no data".to_owned(), |date| date.to_string()),
            );
        }
    }

    println!();
    println!(
        "CloudWatch updates these once a day. Object counts include every version, delete \
         marker, and upload part, so a scan without --versions may list fewer."
    );
}
