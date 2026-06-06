// Mandelbrot Benchmark
// Calculate escape iterations for Mandelbrot set

fn mandelbrot_escape(cr: f64, ci: f64, max_iter: i32) -> i32 {
    let mut zr = 0.0;
    let mut zi = 0.0;
    
    for i in 0..max_iter {
        let zr2 = zr * zr;
        let zi2 = zi * zi;
        
        if zr2 + zi2 > 4.0 {
            return i;
        }
        
        zi = 2.0 * zr * zi + ci;
        zr = zr2 - zi2 + cr;
    }
    
    max_iter
}

fn main() {
    let size = 2000;
    let max_iter = 50;
    let mut total_iter: i64 = 0;
    
    for y in 0..size {
        for x in 0..size {
            let cr = (2.0 * x as f64 / size as f64) - 1.5;
            let ci = (2.0 * y as f64 / size as f64) - 1.0;
            total_iter += mandelbrot_escape(cr, ci, max_iter) as i64;
        }
    }
    
    println!("Total iterations: {}", total_iter);
}
