# Mandelbrot Benchmark
# Calculate escape iterations for Mandelbrot set

def mandelbrot_escape(cr: float, ci: float, max_iter: int) -> int:
    zr = 0.0
    zi = 0.0
    
    for i in range(max_iter):
        zr2 = zr * zr
        zi2 = zi * zi
        
        if zr2 + zi2 > 4.0:
            return i
        
        zi = 2.0 * zr * zi + ci
        zr = zr2 - zi2 + cr
    
    return max_iter

def main():
    size = 2000
    max_iter = 50
    total_iter = 0
    
    for y in range(size):
        for x in range(size):
            cr = (2.0 * x / size) - 1.5
            ci = (2.0 * y / size) - 1.0
            total_iter += mandelbrot_escape(cr, ci, max_iter)
    
    print(f"Total iterations: {total_iter}")

if __name__ == "__main__":
    main()
