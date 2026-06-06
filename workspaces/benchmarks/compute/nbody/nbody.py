# N-Body Benchmark
# Planetary orbit simulation (from Benchmarks Game)

import math

PI = math.pi
SOLAR_MASS = 4 * PI * PI
DAYS_PER_YEAR = 365.24

def make_body(x, y, z, vx, vy, vz, mass):
    return [x, y, z, vx, vy, vz, mass]

def advance(bodies, dt, steps):
    for _ in range(steps):
        # Update velocities
        for i in range(len(bodies)):
            for j in range(i + 1, len(bodies)):
                dx = bodies[i][0] - bodies[j][0]
                dy = bodies[i][1] - bodies[j][1]
                dz = bodies[i][2] - bodies[j][2]
                dist_sq = dx * dx + dy * dy + dz * dz
                dist = math.sqrt(dist_sq)
                mag = dt / (dist_sq * dist)

                bodies[i][3] -= dx * bodies[j][6] * mag
                bodies[i][4] -= dy * bodies[j][6] * mag
                bodies[i][5] -= dz * bodies[j][6] * mag
                bodies[j][3] += dx * bodies[i][6] * mag
                bodies[j][4] += dy * bodies[i][6] * mag
                bodies[j][5] += dz * bodies[i][6] * mag

        # Update positions
        for body in bodies:
            body[0] += dt * body[3]
            body[1] += dt * body[4]
            body[2] += dt * body[5]

def energy(bodies):
    e = 0.0

    for i in range(len(bodies)):
        # Kinetic energy
        e += 0.5 * bodies[i][6] * (
            bodies[i][3] * bodies[i][3] +
            bodies[i][4] * bodies[i][4] +
            bodies[i][5] * bodies[i][5]
        )

        # Potential energy
        for j in range(i + 1, len(bodies)):
            dx = bodies[i][0] - bodies[j][0]
            dy = bodies[i][1] - bodies[j][1]
            dz = bodies[i][2] - bodies[j][2]
            dist = math.sqrt(dx * dx + dy * dy + dz * dz)
            e -= (bodies[i][6] * bodies[j][6]) / dist

    return e

def offset_momentum(bodies):
    px = py = pz = 0.0

    for body in bodies:
        px += body[3] * body[6]
        py += body[4] * body[6]
        pz += body[5] * body[6]

    bodies[0][3] = -px / SOLAR_MASS
    bodies[0][4] = -py / SOLAR_MASS
    bodies[0][5] = -pz / SOLAR_MASS

def main():
    bodies = [
        # Sun
        make_body(0.0, 0.0, 0.0, 0.0, 0.0, 0.0, SOLAR_MASS),
        # Jupiter
        make_body(
            4.84143144246472090,
            -1.16032004402742839,
            -0.103622044471123109,
            0.00166007664274403694 * DAYS_PER_YEAR,
            0.00769901118419740425 * DAYS_PER_YEAR,
            -0.0000690460016972063023 * DAYS_PER_YEAR,
            0.000954791938424326609 * SOLAR_MASS,
        ),
        # Saturn
        make_body(
            8.34336671824457987,
            4.12479856412430479,
            -0.403523417114321381,
            -0.00276742510726862411 * DAYS_PER_YEAR,
            0.00499852801234917238 * DAYS_PER_YEAR,
            0.0000230417297573763929 * DAYS_PER_YEAR,
            0.000285885980666130812 * SOLAR_MASS,
        ),
        # Uranus
        make_body(
            12.8943695621391310,
            -15.1111514016986312,
            -0.223307578892655734,
            0.00296460137564761618 * DAYS_PER_YEAR,
            0.00237847173959480950 * DAYS_PER_YEAR,
            -0.0000296589568540237556 * DAYS_PER_YEAR,
            0.0000436624404335156298 * SOLAR_MASS,
        ),
        # Neptune
        make_body(
            15.3796971148509165,
            -25.9193146099879641,
            0.179258772950371181,
            0.00268067772490389322 * DAYS_PER_YEAR,
            0.00162824170038242295 * DAYS_PER_YEAR,
            -0.0000951592254519715870 * DAYS_PER_YEAR,
            0.0000515138902046611451 * SOLAR_MASS,
        ),
    ]

    offset_momentum(bodies)

    n = 500_000
    advance(bodies, 0.01, n)

    print(f"Final energy: {energy(bodies):.9f}")

if __name__ == "__main__":
    main()
