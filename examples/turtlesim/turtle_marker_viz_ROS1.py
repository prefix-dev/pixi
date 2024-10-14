from math import sin, cos

import rospy
from turtlesim.msg import Pose
from visualization_msgs.msg import Marker


def pose_callback(turtle_pose):
    marker = Marker()

    # Marker settings
    marker.header.frame_id = "map"
    marker.header.stamp = rospy.Time.now()
    marker.ns = "turtle_marker"
    marker.id = 0
    marker.type = Marker.ARROW  # Change to arrow
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

    # Set the color (green arrow in this case)
    marker.color.r = 0.0
    marker.color.g = 1.0
    marker.color.b = 0.0
    marker.color.a = 1.0  # Opaque

    # Lifetime of the marker (0 means it will stay forever)
    marker.lifetime = rospy.Duration()

    # Publish the marker
    marker_pub.publish(marker)


if __name__ == "__main__":
    # Initialize the ROS node
    rospy.init_node("turtle_marker_node", anonymous=True)

    # Publisher for RViz Marker
    marker_pub = rospy.Publisher("/turtle_marker", Marker, queue_size=10)

    # Subscriber for the turtle pose
    rospy.Subscriber("/turtle1/pose", Pose, pose_callback)

    # Keep the node running
    rospy.spin()
