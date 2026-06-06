// N-Body Benchmark
// Planetary orbit simulation (from Benchmarks Game)

use std::f64::consts::PI;

const SOLAR_MASS: f64 = 4.0 * PI * PI;
const DAYS_PER_YEAR: f64 = 365.24;

struct Body {
    x: f64, y: f64, z: f64,
    vx: f64, vy: f64, vz: f64,
    mass: f64,
}

fn advance(bodies: &mut [Body], dt: f64, steps: i32) {
    for _ in 0..steps {
        // Update velocities
        for i in 0..bodies.len() {
            for j in (i + 1)..bodies.len() {
                let dx = bodies[i].x - bodies[j].x;
                let dy = bodies[i].y - bodies[j].y;
                let dz = bodies[i].z - bodies[j].z;
                let dist_sq = dx * dx + dy * dy + dz * dz;
                let dist = dist_sq.sqrt();
                let mag = dt / (dist_sq * dist);

                bodies[i].vx -= dx * bodies[j].mass * mag;
                bodies[i].vy -= dy * bodies[j].mass * mag;
                bodies[i].vz -= dz * bodies[j].mass * mag;
                bodies[j].vx += dx * bodies[i].mass * mag;
                bodies[j].vy += dy * bodies[i].mass * mag;
                bodies[j].vz += dz * bodies[i].mass * mag;
            }
        }

        // Update positions
        for body in bodies.iter_mut() {
            body.x += dt * body.vx;
            body.y += dt * body.vy;
            body.z += dt * body.vz;
        }
    }
}

fn energy(bodies: &[Body]) -> f64 {
    let mut e = 0.0;

    for i in 0..bodies.len() {
        // Kinetic energy
        e += 0.5 * bodies[i].mass * (
            bodies[i].vx * bodies[i].vx +
            bodies[i].vy * bodies[i].vy +
            bodies[i].vz * bodies[i].vz
        );

        // Potential energy
        for j in (i + 1)..bodies.len() {
            let dx = bodies[i].x - bodies[j].x;
            let dy = bodies[i].y - bodies[j].y;
            let dz = bodies[i].z - bodies[j].z;
            let dist = (dx * dx + dy * dy + dz * dz).sqrt();
            e -= (bodies[i].mass * bodies[j].mass) / dist;
        }
    }

    e
}

fn offset_momentum(bodies: &mut [Body]) {
    let mut px = 0.0;
    let mut py = 0.0;
    let mut pz = 0.0;

    for body in bodies.iter() {
        px += body.vx * body.mass;
        py += body.vy * body.mass;
        pz += body.vz * body.mass;
    }

    bodies[0].vx = -px / SOLAR_MASS;
    bodies[0].vy = -py / SOLAR_MASS;
    bodies[0].vz = -pz / SOLAR_MASS;
}

fn main() {
    let mut bodies = [
        // Sun
        Body { x: 0.0, y: 0.0, z: 0.0, vx: 0.0, vy: 0.0, vz: 0.0, mass: SOLAR_MASS },
        // Jupiter
        Body {
            x: 4.84143144246472090,
            y: -1.16032004402742839,
            z: -0.103622044471123109,
            vx: 0.00166007664274403694 * DAYS_PER_YEAR,
            vy: 0.00769901118419740425 * DAYS_PER_YEAR,
            vz: -0.0000690460016972063023 * DAYS_PER_YEAR,
            mass: 0.000954791938424326609 * SOLAR_MASS,
        },
        // Saturn
        Body {
            x: 8.34336671824457987,
            y: 4.12479856412430479,
            z: -0.403523417114321381,
            vx: -0.00276742510726862411 * DAYS_PER_YEAR,
            vy: 0.00499852801234917238 * DAYS_PER_YEAR,
            vz: 0.0000230417297573763929 * DAYS_PER_YEAR,
            mass: 0.000285885980666130812 * SOLAR_MASS,
        },
        // Uranus
        Body {
            x: 12.8943695621391310,
            y: -15.1111514016986312,
            z: -0.223307578892655734,
            vx: 0.00296460137564761618 * DAYS_PER_YEAR,
            vy: 0.00237847173959480950 * DAYS_PER_YEAR,
            vz: -0.0000296589568540237556 * DAYS_PER_YEAR,
            mass: 0.0000436624404335156298 * SOLAR_MASS,
        },
        // Neptune
        Body {
            x: 15.3796971148509165,
            y: -25.9193146099879641,
            z: 0.179258772950371181,
            vx: 0.00268067772490389322 * DAYS_PER_YEAR,
            vy: 0.00162824170038242295 * DAYS_PER_YEAR,
            vz: -0.0000951592254519715870 * DAYS_PER_YEAR,
            mass: 0.0000515138902046611451 * SOLAR_MASS,
        },
    ];

    offset_momentum(&mut bodies);

    let n = 500_000;
    advance(&mut bodies, 0.01, n);

    println!("Final energy: {:.9}", energy(&bodies));
}
