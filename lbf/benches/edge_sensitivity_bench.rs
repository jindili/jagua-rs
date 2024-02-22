use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use criterion::{BenchmarkGroup, BenchmarkId, Criterion, criterion_group, criterion_main};
use criterion::measurement::WallTime;
use itertools::Itertools;
use rand::prelude::SmallRng;
use rand::SeedableRng;
use jagua_rs::entities::instances::bin_packing::BPInstance;
use jagua_rs::entities::instances::instance::Instance;
use jagua_rs::entities::instances::instance_generic::InstanceGeneric;
use jagua_rs::entities::instances::strip_packing::SPInstance;

use jagua_rs::entities::item::Item;
use jagua_rs::entities::problems::problem_generic::{LayoutIndex, ProblemGeneric};
use jagua_rs::geometry::geo_traits::{Shape, TransformableFrom};
use jagua_rs::geometry::primitives::point::Point;
use jagua_rs::geometry::primitives::simple_polygon::SimplePolygon;
use jagua_rs::io::json_instance::JsonInstance;
use lbf::config::Config;
use lbf::io;
use lbf::io::svg_util::SvgDrawOptions;
use lbf::samplers::hpg_sampler::HPGSampler;

use crate::util::{N_ITEMS_REMOVED, SWIM_PATH};

criterion_main!(benches);
criterion_group!(benches, edge_sensitivity_bench_no_ff, edge_sensitivity_bench_with_ff);

mod util;

const EDGE_MULTIPLIERS: [u8; 5] = [1, 2, 4, 8, 16];

const N_TOTAL_SAMPLES: usize = 100_000;
const N_SAMPLES_PER_ITER: usize = 1000;

fn edge_sensitivity_bench_no_ff(c: &mut Criterion){
    let mut config = util::create_base_config();
    config.cde_config.item_surrogate_config.n_ff_poles = 0;
    config.cde_config.item_surrogate_config.n_ff_piers = 0;

    let group = c.benchmark_group("edge_sensitivity_bench_no_ff");
    edge_sensitivity_bench(config, group);
}

fn edge_sensitivity_bench_with_ff(c: &mut Criterion){
    let config = util::create_base_config();
    let group = c.benchmark_group("edge_sensitivity_bench_ff");
    edge_sensitivity_bench(config, group);
}

fn edge_sensitivity_bench(config: Config, mut g: BenchmarkGroup<WallTime>) {
    let json_instance: JsonInstance = serde_json::from_reader(BufReader::new(File::open(SWIM_PATH).unwrap())).unwrap();

    for edge_multiplier in EDGE_MULTIPLIERS {
        let instance = {
            let instance = util::create_instance(&json_instance, config.cde_config, config.poly_simpl_config);
            modify_instance(&instance, edge_multiplier as usize, config)
        };

        let (problem,selected_pi_uids) = util::create_blf_problem(instance.clone(), config, N_ITEMS_REMOVED);

        {
            let draw_options = SvgDrawOptions{
                quadtree: true,
                surrogate: true,
                ..SvgDrawOptions::default()
            };
            let svg = io::layout_to_svg::layout_to_svg(problem.get_layout(LayoutIndex::Existing(0)), &instance, draw_options);
            io::write_svg(&svg, Path::new(&format!("edge_sensitivity_{edge_multiplier}.svg")));
        }

        let mut rng = SmallRng::seed_from_u64(0);

        let layout = problem.get_layout(LayoutIndex::Existing(0));
        /*let samples = {
            let sampler = UniformAARectSampler::new(layout.bin().bbox(), instance.item(0));
            (0..N_SAMPLES).map(
                |_| sampler.sample(&mut rng).compose()
            ).collect_vec()
        };*/

        let samples = {
            let hpg_sampler = HPGSampler::new(instance.item(0), layout).expect("should be able to create HPGSampler");
            (0..N_TOTAL_SAMPLES).map(
                |_| hpg_sampler.sample(&mut rng)
            ).collect_vec()
        };

        let mut samples_cycler = samples.chunks(N_SAMPLES_PER_ITER).cycle();

        let mut n_invalid: i64 = 0;
        let mut n_valid: i64 = 0;

        g.bench_function(BenchmarkId::from_parameter(edge_multiplier), |b| {
            b.iter(|| {
                for i in 0..N_ITEMS_REMOVED {
                    let pi_uid = &selected_pi_uids[i];
                    let item = instance.item(pi_uid.item_id);
                    let mut buffer_shape = item.shape.as_ref().clone();
                    for transf in samples_cycler.next().unwrap() {
                        let collides = match layout.cde().surrogate_collides(item.shape.surrogate(), transf, &[]) {
                            true => true,
                            false => {
                                buffer_shape.transform_from(&item.shape, transf);
                                layout.cde().shape_collides(&buffer_shape, &[])
                            }
                        };
                        match collides {
                            true => n_invalid += 1,
                            false => n_valid += 1
                        }
                    }
                }
            })
        });
        println!("{:.3}% valid", n_valid as f64 / (n_invalid + n_valid) as f64 * 100.0);
    }
    g.finish();
}

fn modify_instance(instance: &Instance, multiplier: usize, config: Config) -> Instance {
    let modified_items = instance.items().iter().map(|(item, qty)| {
        let modified_shape = multiply_edge_count(&item.shape, multiplier);

        let modified_item = Item::new(
            item.id,
            modified_shape,
            item.value,
            item.allowed_rotation.clone(),
            item.centering_transform.clone(),
            item.base_quality,
            config.cde_config.item_surrogate_config
        );
        (modified_item, *qty)
    }).collect_vec();

    match instance {
        Instance::SP(spi) => Instance::SP(SPInstance::new(modified_items, spi.strip_height)),
        Instance::BP(bpi) => Instance::BP(BPInstance::new(modified_items, bpi.bins.clone()))
    }
}
fn multiply_edge_count(shape: &SimplePolygon, multiplier: usize) -> SimplePolygon{
    let mut new_points = vec![];

    for edge in shape.edge_iter(){
        //split x and y into "times" parts
        let x_step = (edge.end().0 - edge.start().0) / multiplier as f64;
        let y_step = (edge.end().1 - edge.start().1) / multiplier as f64;
        let mut start = edge.start();
        for _ in 0..multiplier {
            new_points.push(start);
            start = Point(start.0 + x_step, start.1 + y_step);
        }

    }
    let new_polygon = SimplePolygon::new(new_points);
    assert!(almost::equal(shape.area(), new_polygon.area()));
    new_polygon
}