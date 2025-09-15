# pyright: reportUntypedBaseClass=false, reportUnannotatedClassAttribute=false

from math import sin, cos
import rclpy
from rclpy.node import Node
from turtlesim.msg import Pose
from visualization_msgs.msg import Marker
from builtin_interfaces.msg import Duration


class TurtleMarkerNode(Node):
    def __init__(self):
        super().__init__("turtle_marker_node")

        # Create publisher for RViz Marker
        self.marker_pub = self.create_publisher(Marker, "/turtle_marker", 10)

        # Create subscriber for turtle pose
        self.create_subscription(Pose, "/turtle1/pose", self.pose_callback, 10)

    def pose_callback(self, turtle_pose):
        marker = Marker()

        # Marker settings
        marker.header.frame_id = "map"
        marker.header.stamp = self.get_clock().now().to_msg()
        marker.ns = "turtle_marker"
        marker.id = 0
        marker.type = Marker.ARROW  # Arrow marker
        marker.action = Marker.ADD

        # Set the position of the marker based on turtle's pose
        marker.pose.position.x = turtle_pose.x - 5.5
        marker.pose.position.y = turtle_pose.y - 5.5
        marker.pose.position.z = 0.0

        # Set the orientation (turtle's heading in radians)
        marker.pose.orientation.x = 0.0
        marker.pose.orientation.y = 0.0
        marker.pose.orientation.z = sin(turtle_pose.theta / 2.0)
        marker.pose.orientation.w = cos(turtle_pose.theta / 2.0)

        # Set the scale of the arrow (length and width of the arrow)
        marker.scale.x = 1.0  # Arrow length
        marker.scale.y = 0.2  # Arrow width
        marker.scale.z = 0.2  # Arrow height

        # Set the color (green arrow)
        marker.color.r = 0.0
        marker.color.g = 1.0
        marker.color.b = 0.0
        marker.color.a = 1.0  # Opaque

        # Lifetime of the marker (0 means it will stay forever)
        marker.lifetime = Duration()

        # Publish the marker
        self.marker_pub.publish(marker)


def main(args=None):
    rclpy.init(args=args)
    node = TurtleMarkerNode()

    # Keep the node spinning
    rclpy.spin(node)

    # Shutdown once done
    node.destroy_node()
    rclpy.shutdown()


if __name__ == "__main__":
    main()
