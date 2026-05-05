import math
from typing import Any

import rclpy  # type: ignore[import-not-found]
from geometry_msgs.msg import Point, Twist  # type: ignore[import-not-found]
from rclpy.node import Node  # type: ignore[import-not-found]
from turtlesim.msg import Pose  # type: ignore[import-not-found]


class TurtleNavigator(Node):  # type: ignore[misc]
    def __init__(self) -> None:
        super().__init__(node_name="turtle_navigator")
        self.x_goal = 5.0
        self.y_goal = 5.0
        self.kp = 1.0
        self.ki = 0.0
        self.kd = 0.05
        self.prev_error = 0.0
        self.integral = 0.0

        self.subscription = self.create_subscription(Point, "coordinates", self.goal_callback, 10)
        self.pose_subscription = self.create_subscription(
            Pose, "turtle1/pose", self.pose_callback, 10
        )
        self.publisher = self.create_publisher(Twist, "turtle1/cmd_vel", 10)

        self.timer = self.create_timer(0.1, self.control_loop)

        self.x_current = 0.0
        self.y_current = 0.0
        self.theta_current = 0.0

    def goal_callback(self, msg: Any) -> None:
        self.x_goal = msg.x
        self.y_goal = msg.y
        self.get_logger().info(f"Received goal: x={self.x_goal}, y={self.y_goal}")

    def pose_callback(self, msg: Any) -> None:
        self.x_current = msg.x
        self.y_current = msg.y
        self.theta_current = msg.theta

    def control_loop(self) -> None:
        error_x = self.x_goal - self.x_current
        error_y = self.y_goal - self.y_current
        distance_error = math.sqrt(error_x**2 + error_y**2)

        angle_to_goal = math.atan2(error_y, error_x)
        angle_error = angle_to_goal - self.theta_current

        # Normalize angle error to the range [-pi, pi]
        while angle_error > math.pi:
            angle_error -= 2 * math.pi
        while angle_error < -math.pi:
            angle_error += 2 * math.pi

        # PID control
        control_signal = (
            self.kp * distance_error
            + self.ki * self.integral
            + self.kd * (distance_error - self.prev_error)
        )
        self.integral += distance_error
        self.prev_error = distance_error

        # Limit control signal
        max_linear_speed = 2.0  # Max linear speed
        max_angular_speed = 2.0  # Max angular speed
        control_signal = max(min(control_signal, max_linear_speed), -max_linear_speed)

        # Publish velocity commands
        msg = Twist()
        msg.linear.x = control_signal
        msg.angular.z = 4.0 * angle_error  # Simple P controller for angle
        msg.angular.z = max(min(msg.angular.z, max_angular_speed), -max_angular_speed)

        self.publisher.publish(msg)


def main(args: Any = None) -> None:
    rclpy.init(args=args)
    print("Turtle Navigator")
    turtle_navigator = TurtleNavigator()
    print("Waiting for command..")
    rclpy.spin(turtle_navigator)
    turtle_navigator.destroy_node()
    rclpy.shutdown()


if __name__ == "__main__":
    main()
