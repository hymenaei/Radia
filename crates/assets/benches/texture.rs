use std::{hint::black_box, thread};

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use jpeg2k::{DecodeParameters, Image, ImagePixelData};

fn decode(input: &[u8], edge: u32) -> Vec<u8> {
    let reduce = 4096u32.ilog2() - edge.ilog2();
    let image = Image::from_bytes_with(input, DecodeParameters::new().reduce(reduce).strict(true)).expect("committed JPEG2000 fixture");
    let ImagePixelData::L8(pixels) = image.get_pixels(None).expect("committed grayscale pixels").data else {
        panic!("expected grayscale fixture");
    };
    pixels
}

fn textures(c: &mut Criterion) {
    let input = include_bytes!("../tests/fixtures/gray-4096.j2c");
    let mut group = c.benchmark_group("texture/jpeg2000");
    for edge in [512u32, 1024, 2048] {
        for workers in [1u64, 2] {
            group.throughput(Throughput::Elements(u64::from(edge).pow(2) * workers));
            group.bench_function(BenchmarkId::new(format!("gray_4096/{workers}_workers"), edge), |b| {
                b.iter(|| {
                    if workers == 1 {
                        black_box(decode(black_box(input), edge));
                    } else {
                        thread::scope(|scope| {
                            let other = scope.spawn(|| decode(black_box(input), edge));
                            black_box(decode(black_box(input), edge));
                            black_box(other.join().expect("decoder worker completes"));
                        });
                    }
                });
            });
        }
    }
    group.finish();
}

criterion_group!(benches, textures);
criterion_main!(benches);
