use rand::rngs::StdRng;
use rand::SeedableRng;
use rand_distr::{Normal, Distribution};
use futuresdr::prelude::*;

#[derive(Block)]
pub struct AddAWGN<
  I = DefaultCpuReader<Complex32>,
  O = DefaultCpuWriter<Complex32>,
>
where
    I: CpuBufferReader<Item = Complex32>,
    O: CpuBufferWriter<Item = Complex32>,
{
    #[input]
    input: I,
    #[output]
    output: O,

    noise: Normal<f32>,
    rng: StdRng,
}

impl<I,O> AddAWGN<I, O> 
where
    I: CpuBufferReader<Item = Complex32>,
    O: CpuBufferWriter<Item = Complex32>,
{

    pub fn new(sigma: f32, seed: u64) -> Self {
        let noise = Normal::<f32>::new(0.0, sigma).unwrap();
        let rng = StdRng::seed_from_u64(seed);
        Self {
            input : I::default(),
            output: O::default(),
            noise: noise,
            rng: rng,
        }
    }
}

impl<I, O> Kernel for AddAWGN<I, O>
where
    I: CpuBufferReader<Item = Complex32>,
    O: CpuBufferWriter<Item = Complex32>,
{
    async fn work(
        &mut self,
        io: &mut WorkIo,
        _mio: &mut MessageOutputs,
        _b: &mut BlockMeta,
    ) -> Result<()> {

        let i = self.input.slice();
        let o = self.output.slice();
        let i_len = i.len();

        let m = std::cmp::min(i.len(), o.len());
        if m > 0 {
            for j in 0..m {
                let n_re = self.noise.sample(&mut self.rng);
                let n_im = self.noise.sample(&mut self.rng);
                o[j] = i[j] + Complex32::new(n_re, n_im);
            }
            self.input.consume(m);
            self.output.produce(m);
        }

        if self.input.finished() && m == i_len {
            io.finished = true;
        }

        Ok(())
    }
}
