use gpucachesim::exec::{
    alloc,
    model::{Dim, MemorySpace},
    tracegen::{self, TraceGenerator, Tracer},
    Kernel, ThreadBlock, ThreadIndex,
};
use num_traits::{Float, Zero};
use rand::{
    distributions::{self, Distribution},
    Rng,
};
use tokio::sync::Mutex;

#[derive(Debug)]
struct Matrixmul<'a, T> {
    dev_a: Mutex<alloc::DevicePtr<&'a Vec<T>>>,
    dev_b: Mutex<alloc::DevicePtr<&'a Vec<T>>>,
    dev_result: Mutex<alloc::DevicePtr<&'a mut Vec<T>>>,
    num_rows: usize,
    /// Shared memory array used to store the sub-matrix of A
    shared_mem_a: Mutex<alloc::DevicePtr<Vec<T>>>,
    /// Shared memory array used to store the sub-matrix of B
    shared_mem_b: Mutex<alloc::DevicePtr<Vec<T>>>,
    /// Block size
    block_size: usize,
}

#[async_trait::async_trait]
impl<'a, T> Kernel for Matrixmul<'a, T>
where
    T: Float + std::ops::AddAssign + Send + Sync + std::fmt::Debug,
{
    type Error = std::convert::Infallible;

    #[gpucachesim::exec::instrument_control_flow]
    async fn run(&self, block: &ThreadBlock, tid: &ThreadIndex) -> Result<(), Self::Error> {
        let bx = tid.block_idx.x as usize;
        let by = tid.block_idx.y as usize;

        let tx = tid.thread_idx.x as usize;
        let ty = tid.thread_idx.y as usize;

        // index of the first sub-matrix of A processed by the block
        let a_begin = self.num_rows * self.block_size * by;

        // index of the last sub-matrix of A processed by the block
        let a_end = a_begin + self.num_rows - 1;

        // step size used to iterate through the sub-matrices of A
        let a_step = self.block_size;

        // index of the first sub-matrix of B processed by the block
        let b_begin = self.block_size * bx;

        // step size used to iterate through the sub-matrices of B
        let b_step = self.block_size * self.num_rows;

        // c_sub is used to store the element of the block sub-matrix
        // that is computed by the thread
        let mut c_sub = T::zero();

        // Loop over all the sub-matrices of A and B
        // required to compute the block sub-matrix

        let mut ai = a_begin;
        let mut bi = b_begin;
        while ai <= a_end {
            {
                // load the matrices from device memory to shared memory
                // each thread loads one element of each matrix

                // As[ty][tx] = A[a + wA * ty + tx];
                let a = self.dev_a.lock().await;
                let mut shared_a = self.shared_mem_a.lock().await;
                shared_a[(tid, ty * self.block_size + tx)] = a[(tid, ai + self.num_rows * ty + tx)];

                // Bs[ty][tx] = B[b + wB * ty + tx];
                let b = self.dev_b.lock().await;
                let mut shared_b = self.shared_mem_b.lock().await;
                shared_b[(tid, ty * self.block_size + tx)] = b[(tid, bi + self.num_rows * ty + tx)];
            }

            block.synchronize_threads().await;

            // make sure shared mem has been loaded
            // {
            //     let shared_a = self.shared_mem_a.lock().await;
            //     dbg!(&shared_a);
            //     assert!(shared_a.inner.iter().all(|x| *x != T::zero()));
            //
            //     let shared_b = self.shared_mem_b.lock().await;
            //     dbg!(&shared_b);
            //     assert!(shared_b.inner.iter().all(|x| *x != T::zero()));
            // }

            // lets not do anything here
            for k in 0..self.block_size {
                let shared_a = self.shared_mem_a.lock().await;
                let shared_b = self.shared_mem_b.lock().await;
                c_sub += shared_a[(tid, ty * self.block_size + k)]
                    * shared_b[(tid, k * self.block_size + tx)];
            }

            block.synchronize_threads().await;

            ai += a_step;
            bi += b_step;
        }

        // Write the block sub-matrix to device memory;
        // each thread writes one element
        let c = self.num_rows * self.block_size * by + self.block_size * bx;
        let mut result = self.dev_result.lock().await;
        result[(tid, c + self.num_rows * ty + tx)] = c_sub;

        Ok(())
    }

    fn name(&self) -> Option<&str> {
        Some("matrixmul")
    }
}

pub fn reference<T>(a: &Vec<T>, b: &Vec<T>, result: &mut Vec<T>, size: usize)
where
    T: Float + std::ops::AddAssign,
{
    assert_eq!(a.len(), b.len());
    assert_eq!(a.len(), result.len());
    assert_eq!(a.len(), size * size);
    for i in 0..size {
        for j in 0..size {
            let mut sum = T::zero();
            for k in 0..size {
                sum += a[i * size + k] * b[k * size + j];
            }

            result[i * size + j] = sum;
        }
    }
}

/// Matrixmul benchmark application.
pub async fn benchmark<T>(num_rows: usize) -> super::Result
where
    T: Float + Zero + std::ops::AddAssign + Send + Sync + std::fmt::Debug,
    distributions::Open01: Distribution<T>,
{
    let mut rng = rand::thread_rng();

    let block_size = 32; // 32 x 32
    let matrix_size = num_rows * num_rows;
    // create host vectors
    let mut a: Vec<T> = vec![T::zero(); matrix_size];
    let mut b: Vec<T> = vec![T::zero(); matrix_size];
    let mut result: Vec<T> = vec![T::zero(); matrix_size];

    // initialize vectors
    for i in 0..matrix_size {
        a[i] = T::one() + rng.sample(distributions::Open01);
        b[i] = T::one() + rng.sample(distributions::Open01);
    }

    matrixmul(&a, &b, &mut result, num_rows, block_size).await
}

pub async fn matrixmul<T>(
    a: &Vec<T>,
    b: &Vec<T>,
    result: &mut Vec<T>,
    num_rows: usize,
    block_size: usize,
) -> super::Result
where
    T: Float + Zero + std::ops::AddAssign + Send + Sync + std::fmt::Debug,
{
    let tracer = Tracer::new();

    assert_eq!(a.len(), b.len());
    assert_eq!(b.len(), result.len());
    let _n = a.len();

    // allocate memory for each vector on simulated GPU device
    let dev_a = tracer
        .allocate(
            a,
            Some(alloc::Options {
                mem_space: MemorySpace::Global,
                name: Some("a".to_string()),
                ..alloc::Options::default()
            }),
        )
        .await;
    let dev_b = tracer
        .allocate(
            b,
            Some(alloc::Options {
                mem_space: MemorySpace::Global,
                name: Some("b".to_string()),
                ..alloc::Options::default()
            }),
        )
        .await;
    let dev_result = tracer
        .allocate(
            result,
            Some(alloc::Options {
                mem_space: MemorySpace::Global,
                name: Some("result".to_string()),
                ..alloc::Options::default()
            }),
        )
        .await;

    // shared memory
    let shared_mem_a = vec![T::zero(); block_size * block_size];
    let shared_mem_a = tracer
        .allocate(
            shared_mem_a,
            Some(alloc::Options {
                mem_space: MemorySpace::Shared,
                name: Some("shared_a".to_string()),
                ..alloc::Options::default()
            }),
        )
        .await;

    let shared_mem_b = vec![T::zero(); block_size * block_size];
    let shared_mem_b = tracer
        .allocate(
            shared_mem_b,
            Some(alloc::Options {
                mem_space: MemorySpace::Shared,
                name: Some("shared_b".to_string()),
                ..alloc::Options::default()
            }),
        )
        .await;

    // number of thread blocks in grid
    let block_dim: Dim = (block_size as u32, block_size as u32).into();
    let grid_size = (num_rows + (block_size - 1)) / block_size;
    let grid_dim: Dim = (grid_size as u32, grid_size as u32).into();
    println!("grid dim:  {grid_dim}");
    println!("block dim: {block_dim}");

    assert!(grid_dim.x > 0);
    assert!(grid_dim.y > 0);
    assert!(grid_dim.z > 0);

    let mut kernel: Matrixmul<T> = Matrixmul {
        dev_a: Mutex::new(dev_a),
        dev_b: Mutex::new(dev_b),
        dev_result: Mutex::new(dev_result),
        shared_mem_a: Mutex::new(shared_mem_a),
        shared_mem_b: Mutex::new(shared_mem_b),
        num_rows,
        block_size,
    };
    let mut options = tracegen::Options::default();
    options.no_data_dependency = false;
    let trace = tracer
        .trace_kernel(grid_dim, block_dim, &mut kernel, &options)
        .await?;
    Ok((tracer.commands().await, vec![trace]))
}

#[cfg(test)]
mod tests {
    use color_eyre::eyre;
    use gpucachesim::exec::tracegen::fmt;
    use ndarray::Array2;
    use rand::Rng;

    const EPSILON: f32 = 0.0001;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_correctness() -> eyre::Result<()> {
        crate::tests::init_test();
        let mut rng = rand::thread_rng();

        let block_size = 4;
        let size = 4;
        let matrix_size = size * size;
        let matrix_shape = (size, size);

        // create host vectors
        let mut a: Vec<f32> = vec![0.0; matrix_size];
        let mut b: Vec<f32> = vec![0.0; matrix_size];
        let mut result: Vec<f32> = vec![0.0; matrix_size];
        let mut ref_result: Vec<f32> = vec![0.0; matrix_size];

        // initialize random matrix
        for i in 0..matrix_size {
            a[i] = 1.0 + rng.gen::<f32>();
            b[i] = 1.0 + rng.gen::<f32>();
        }

        let ndarray_result = {
            let ref_a = Array2::from_shape_vec(matrix_shape, a.clone())?;
            let ref_b = Array2::from_shape_vec(matrix_shape, b.clone())?;
            ref_a.dot(&ref_b)
        };
        let (_commands, kernel_traces) =
            super::matrixmul(&a, &b, &mut result, size, block_size).await?;
        assert_eq!(kernel_traces.len(), 1);
        let (_launch_config, trace) = kernel_traces.into_iter().next().unwrap();

        // print the trace
        let warp_traces = trace.clone().to_warp_traces();
        let first_warp = &warp_traces[&(trace_model::Dim::ZERO, 0)];

        let simplified_trace: Vec<_> = fmt::simplify_warp_trace(&first_warp, true).collect();
        for inst in &simplified_trace {
            println!("{}", inst);
        }

        // check for correctness
        super::reference(&a, &b, &mut ref_result, size);

        let ref_result = Array2::from_shape_vec(matrix_shape, ref_result)?;
        let result = Array2::from_shape_vec(matrix_shape, result)?;
        dbg!(&ndarray_result);
        dbg!(&ref_result);
        dbg!(&result);

        if !approx::abs_diff_eq!(ref_result, ndarray_result, epsilon = EPSILON) {
            diff::assert_eq!(have: ref_result, want: ndarray_result);
        }
        if !approx::abs_diff_eq!(result, ndarray_result, epsilon = EPSILON) {
            diff::assert_eq!(have: result, want: ndarray_result);
        }

        Ok(())
    }
}
