# OpenCV example
OpenCV is a powerful tool to do computer vision and fully opensource.

Here are some example on how to use it with `pixi`.

## Simple face detection algorithm
```shell
pixi run start
```
![Face detection result](https://github.com/ruben-arts/pixi/assets/12893423/c0151496-caae-407c-9e90-0f71f3c19aa7)


## Simple camera calibration script
```shell
pixi run calibrate
```

You'll need a checkerboard for this to work.
Print this: [![chessboard](https://github.com/opencv/opencv/blob/4.x/doc/pattern.png?raw=true)](https://github.com/opencv/opencv/blob/4.x/doc/pattern.png)

To make a picture for calibration press `SPACE`
Do this approximately 10 times with the chessboard in view of the camera

After that press `ESC` which will start the calibration.

When the calibration is done the camera will be used again to find the distance to the checkerboard.

![calibrated camera result](https://github.com/ruben-arts/pixi/assets/12893423/f42825d7-5010-4805-9f6b-b02075395413)
