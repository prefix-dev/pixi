from math import tau

import numpy as np
import rerun as rr
from rerun_demo.data import build_color_spiral
from rerun_demo.util import bounce_lerp

NUM_POINTS = 100

rr.init("DNA abacus")
rr.spawn()
rr.set_time_seconds("stable_time", 0)

# points and colors are both np.array((NUM_POINTS, 3))
points1, colors1 = build_color_spiral(NUM_POINTS)
points2, colors2 = build_color_spiral(NUM_POINTS, angular_offset=tau*0.5)

rr.log("dna/structure/left", rr.Points3D(points1, colors=colors1, radii=0.08))
rr.log("dna/structure/right", rr.Points3D(points2, colors=colors2, radii=0.08))

rr.log(
    "dna/structure/scaffolding",
    rr.LineStrips3D(np.stack((points1, points2), axis=1), colors=[128, 128, 128])
)

time_offsets = np.random.rand(NUM_POINTS)
for i in range(400):
    time = i * 0.01
    rr.set_time_seconds("stable_time", time)

    times = np.repeat(time, NUM_POINTS) + time_offsets
    beads = [bounce_lerp(points1[n], points2[n], times[n]) for n in range(NUM_POINTS)]
    colors = [[int(bounce_lerp(80, 230, times[n] * 2))] for n in range(NUM_POINTS)]
    rr.log(
        "dna/structure/scaffolding/beads",
        rr.Points3D(beads, radii=0.06, colors=np.repeat(colors, 3, axis=-1)),
    )

    rr.log(
        "dna/structure",
        rr.Transform3D(rotation=rr.RotationAxisAngle(
            axis=[0, 0, 1],
            radians=time / 4.0 * tau)),
    )
