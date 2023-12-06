import rerun as rr
import networkx as nx
import yaml
import numpy as np
import hashlib
import sys

# Give relative path or default to local pixi.lock
lockfile_path = sys.argv[1] if len(sys.argv) > 1 else 'pixi.lock'

with open(lockfile_path, 'r') as file:
    lockfile_data = yaml.safe_load(file)

package_data = lockfile_data['package']
package_names = [package['name'] for package in package_data]

graph = nx.DiGraph()
for package in package_data:
    package_name = package['name']
    dependencies = package.get('dependencies', [])
    graph.add_node(package_name)
    for i, dep in enumerate(dependencies):
        graph.add_edge(package_name, dep.split(" ")[0])

rr.init("fdg", spawn=True)
rr.connect()

# Force-Directed Simulation Parameters
iterations = 100
repulsive_force = 0.04
attractive_force = 0.002


def hash_string_to_int(string):
    return int(hashlib.sha256(string.encode('utf-8')).hexdigest(), 16) % (10 ** 8)


# Memoization dictionary
color_cache = {}


# Function to get color
def get_color_for_node(node):
    if node not in color_cache:
        np.random.seed(hash_string_to_int(node))
        color_cache[node] = np.random.rand(3)  # Generate and store color
    return color_cache[node]


def apply_forces_and_log(graph, pos):
    damping = 0.9
    max_force = 10
    degree_scale = 0.9  # Scale factor for degree-based forces

    for iteration in range(iterations):
        force = {node: np.zeros(3) for node in graph}

        # Degree-based repulsive forces
        for i, node1 in enumerate(graph):
            for node2 in list(graph)[i + 1:]:
                diff = pos[node1] - pos[node2]
                dist = np.linalg.norm(diff) + 1e-9  # Avoid divide-by-zero
                degree_factor = (
                        (graph.degree(node1) + graph.degree(node2)) * degree_scale)
                repel = repulsive_force * degree_factor / dist ** 2
                force_vector = repel * diff  # / dist
                force[node1] += np.clip(force_vector, -max_force, max_force)
                force[node2] -= np.clip(force_vector, -max_force, max_force)

        # Degree-based attractive forces
        for edge in graph.edges():
            u, v = edge
            diff = pos[u] - pos[v]
            dist = np.linalg.norm(diff)
            if dist > 0:
                degree_factor = (graph.degree(u) + graph.degree(v)) * degree_scale
                attract = (attractive_force * dist ** 2) / degree_factor
                force[u] -= attract * diff / dist
                force[v] += attract * diff / dist

        # Update positions with damping
        for node in graph:
            pos[node] += force[node] * damping
            position = np.array(pos[node])
            color = get_color_for_node(node)  # Retrieve color, memoized
            rr.log(f"graph_nodes/{node}",
                   rr.Points3D([position],
                               colors=[color],
                               radii=max(graph.degree(node) / 20, 0.5)),
                   rr.AnyValues(node))

        edges_array = np.array([[pos[u], pos[v]] for u, v in graph.edges()])

        # Log the edges array
        rr.log("graph_nodes/graph_edges",
               rr.LineStrips3D(edges_array, radii=0.02, colors=[1, 1, 1, 0.1]))

    return pos


# Identify the node with the highest degree
central_node = max(graph.degree, key=lambda x: x[1])[0]

# Initial positions with the central node at the center
initial_pos = nx.spring_layout(graph, dim=3)
initial_pos[central_node] = np.array([0.5, 0.5, 0.5])  # Center position

# Apply the force-directed simulation
final_pos = apply_forces_and_log(graph, initial_pos)
