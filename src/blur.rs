extern crate test;
use std::{future::Future, ops::Index};

use serde::{Deserialize, Serialize};
use test::{black_box, Bencher};

use crate::{GraphBuilder, Symbol, Task};

fn sample(dim: &[usize; 2]) -> Vec<f32> {
    let mut sample = vec![0f32; dim[0] * dim[1]];
    for x in 0..dim[0] {
        let k = dim[1] as f32 / 2.;
        let y = (x as f32 * 0.1).sin() * k + k;
        sample[x + y as usize * dim[0]] = 1.;
    }
    sample
}

fn table(img: &impl Index<usize, Output = f32>, dim: &[usize]) {
    let brightness = [".", ":", ";", "!", "|", "?", "&", "=", "%", "#", "@"];

    for y in 0..dim[1] {
        for x in 0..dim[0] {
            let n = img[x + y * dim[0]];
            //print!(" {:.3}", n); // numeric
            print!("{}", brightness[(n * 20.).min(10.) as usize]); // visual
        }
        println!();
    }
    println!();
}

#[bench]
fn basic_blur(b: &mut Bencher) {
    fn blur_trans(img: &[f32], output: &mut [f32], dim: &[usize; 2]) {
        let img = |x: isize, y: isize| {
            if x < 0 || x >= dim[0] as isize || y < 0 || y >= dim[1] as isize {
                0f32
            } else {
                img[x as usize + y as usize * dim[0]]
            }
        };

        let [x, y] = dim;
        for y in 0..*y as isize {
            for x in 0..*x as isize {
                let p = (img(x + 1, y) + img(x - 1, y) + img(x, y)) / 3.;
                // output[x as usize + y as usize * dim[0]] = p; // non-transposed output
                output[y as usize + x as usize * dim[1]] = p; // transposed output
            }
        }
    }

    //#[rustfmt::skip]
    //let input = black_box([
    //  0f32,0f32,0f32,0f32,0f32,0f32,
    //  0f32,0f32,0f32,0f32,0f32,0f32,
    //  0f32,0f32,1f32,1f32,0f32,0f32,
    //  0f32,0f32,0f32,1f32,0f32,0f32,
    //  0f32,0f32,0f32,0f32,0f32,0f32,
    //  0f32,0f32,0f32,0f32,0f32,0f32,
    //]);

    let input = black_box(sample(&DIM));
    let mut output = black_box([0f32; DIM[0] * DIM[1]]);
    let mut horizontal = black_box([0f32; DIM[0] * DIM[1]]);

    b.iter(|| {
        let trans = [DIM[1], DIM[0]];
        blur_trans(&input, &mut horizontal, &DIM);
        //blur_x(&input, &mut horizontal, &trans);
        blur_trans(&horizontal, &mut output, &trans);
        //blur_x(&horizontal, &mut output, &DIM);

        black_box(output);
    });

    table(&input, &DIM);
    table(&output, &DIM);
}

struct Const<T>(*const T);
impl<T: Sync + Serialize + Deserialize<'static> + 'static> Task for Const<T> {
    type InitOutput = Symbol<T>;
    type Output = T;

    fn init(self, graph: &mut GraphBuilder<Self>) -> Self::InitOutput {
        graph.mutate_node(|node| unsafe {
            let b = &mut *node.output.get();
            b.data = self.0 as *mut ();
            b.drop = |_| {};
        });
        graph.this_node()
    }
}

#[derive(Clone, Copy)]
struct Matrix(*const f32, [usize; 2]);
unsafe impl Send for Matrix {}
impl Index<[usize; 2]> for Matrix {
    type Output = f32;

    fn index(&self, index: [usize; 2]) -> &Self::Output {
        let [x, y] = index;
        assert!(x < self.1[0] && y < self.1[1]);
        unsafe { &*self.0.add(x + y * self.1[0]) }
    }
}
impl Matrix {
    fn columns(&self) -> usize {
        self.1[0]
    }
    fn rows(&self) -> usize {
        self.1[1]
    }
}

struct MorphicBlur<'a>(&'a Vec<f32>, &'a mut Vec<f32>, &'a mut Vec<f32>, [usize; 2]);
struct MorphicBlurStage {
    input: Symbol<Vec<f32>>,
    dim: [usize; 2],
    bound: [[usize; 2]; 2],
}

impl Task for MorphicBlurStage {
    type InitOutput = Symbol<Vec<f32>>;
    type Output = Vec<f32>;

    fn init(self, graph: &mut GraphBuilder<Self>) -> Self::InitOutput {
        let source = graph.lock_symbol(self.input);
        graph.task(Box::new(move |graph, node| {
            let source = source.clone().own(graph);
            Box::pin(async move {
                let m = unsafe { Matrix((*source.await.0).as_ptr(), self.dim) };

                for y in self.bound[0][1]..self.bound[1][1] {
                    for x in self.bound[0][0]..self.bound[1][0] {
                        let p = (m[[x + 1, y]] + m[[x - 1, y]] + m[[x, y]]) / 3.;
                        unsafe {
                            (*node.output::<Vec<f32>>())[x * m.rows() + y] = p;
                        }
                    }
                }
            })
        }));
        graph.this_node()
    }
}

impl<'a> Task for MorphicBlur<'a> {
    type InitOutput = ();
    type Output = ();

    fn init(self, graph: &mut GraphBuilder<Self>) -> Self::InitOutput {
        let dim = self.3;

        let input = graph.spawn(Const(self.0), None);

        let chunks = 1;
        let chunk = (dim[1] - 1) / chunks;
        let n = 1;

        //for n in 1..chunks + 1 {
        let stage1 = graph.spawn(
            MorphicBlurStage {
                input: input.clone(),
                dim,
                bound: [[1, 1], [dim[0] - 1, n * chunk]],
            },
            Some(self.1),
        );

        let output = graph.spawn(
            MorphicBlurStage {
                input: stage1.clone(),
                dim: [dim[1], dim[0]],
                bound: [[1, 1], [n * chunk, dim[0] - 1]],
            },
            Some(self.2),
        );
        //}

        //let input = graph.lock_symbol(input);
        //let stage1 = graph.lock_symbol(stage1);
        let output = graph.lock_symbol(output);

        task!(graph, (output/*, input, stage1*/), {
            //table(unsafe { &*input.await.0 }, &dim);
            //table(unsafe { &*stage1.await.0 }, &[dim[1], dim[0]]);
            //table(unsafe { &*output.await.0 }, &dim);
            black_box(unsafe { &*output.await.0 });
        })
    }
}

fn morphic_blur<'a>(
    input: &'a Vec<f32>,
    stage1: &'a mut Vec<f32>,
    output: &'a mut Vec<f32>,
    dim: &[usize; 2],
) -> MorphicBlur<'a> {
    MorphicBlur(input, stage1, output, *dim)
}

#[bench]
fn morphic(b: &mut Bencher) {
    let padded_dim = [DIM[0] + 2, DIM[1] + 2];
    let input = sample(&padded_dim);
    let mut stage1 = vec![0f32; padded_dim[0] * padded_dim[1]];
    let mut output = vec![0f32; padded_dim[0] * padded_dim[1]];

    let builder = GraphBuilder::main(morphic_blur(&input, &mut stage1, &mut output, &padded_dim));
    let graph = builder.build();
    let (net_events, mut net) = graph.init_net();
    let lock = std::sync::Mutex::new(());
    b.iter(|| {
        let lock = lock.lock();
        graph.spin_down();
        graph.realize(net_events.clone());
        net.run();

        drop(lock);
    });
    println!("Finishing test...");
    graph.kill(net.kill());
}

const DIM: [usize; 2] = [100, 50];