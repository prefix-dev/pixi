from particle_kit import View, cpp, rs

view = View(width=800, height=600, title="Polyglot Particles")

emitters = [
    cpp.Cone(
        x=400,
        y=520,
        angle=-1.5707963,
        spread=0.6,
        rate=180,
        speed=220,
        lifetime=2.5,
        size=3,
        color=0xFFAA22FF,
    ),
    rs.Ring(
        x=200,
        y=200,
        radius=60,
        rate=120,
        speed=70,
        lifetime=3.0,
        size=2,
        color=0x44CCFFFF,
    ),
]

modifiers = [
    cpp.Gravity(ay=180),
    cpp.Drag(k=0.4),
    rs.Vortex(x=600, y=300, strength=2.0, falloff=180),
]

view.run(emitters=emitters, modifiers=modifiers, capacity=8000, fps=60)
