import cv2
import numpy as np

# Termination criteria
criteria = (cv2.TERM_CRITERIA_EPS + cv2.TERM_CRITERIA_MAX_ITER, 10, 0.001)

# Prepare object points
# The example chessboard is 9x6.
CHESSBOARD_X = 6
CHESSBOARD_Y = 9
# The example chessboard printed on a A4 paper will approximately be 22.5mm.
SQUARE_SIZE_MM = 22.5

objp = np.zeros((CHESSBOARD_X * CHESSBOARD_Y, 3), np.float32)
objp[:, :2] = np.mgrid[0:CHESSBOARD_Y, 0:CHESSBOARD_X].T.reshape(-1, 2) * (
    SQUARE_SIZE_MM * 0.001
)

# Arrays to store object points and image points
objpoints = []
imgpoints = []

# Initialize the camera
cap = cv2.VideoCapture(0)

# Set the frame width and height
cap.set(cv2.CAP_PROP_FRAME_WIDTH, 1280)
cap.set(cv2.CAP_PROP_FRAME_HEIGHT, 720)

img_counter = 0

print("Press SPACE to capture an image for calibration.")
print("Press ESC to calibrate the camera using the previously captured images.")

while True:
    ret, frame = cap.read()
    if not ret:
        break

    frame_clean = cv2.copyTo(frame, None)
    # Find the chess board corners
    gray = cv2.cvtColor(frame, cv2.COLOR_BGR2GRAY)
    ret, corners = cv2.findChessboardCorners(gray, (CHESSBOARD_Y, CHESSBOARD_X), None)

    if ret:
        cv2.drawChessboardCorners(frame, (CHESSBOARD_Y, CHESSBOARD_X), corners, ret)

    cv2.imshow("Test", frame)
    k = cv2.waitKey(1)

    if k % 256 == 27:
        # ESC pressed
        print("Escape hit, closing...")
        break
    elif k % 256 == 32:
        # SPACE pressed
        img_name = f"opencv_frame_{img_counter}.png"
        cv2.imwrite(img_name, frame_clean)
        print(f"{img_name} written!")
        img_counter += 1

        # Convert to grayscale
        gray = cv2.cvtColor(frame_clean, cv2.COLOR_BGR2GRAY)

        # Find the chess board corners
        ret, corners = cv2.findChessboardCorners(
            gray, (CHESSBOARD_Y, CHESSBOARD_X), None
        )

        # If found, add object points, image points (after refining them)
        if ret:
            objpoints.append(objp)

            corners2 = cv2.cornerSubPix(gray, corners, (11, 11), (-1, -1), criteria)
            imgpoints.append(corners2)
        else:
            print("No chessboard detected in this image: {img_name}")

cv2.destroyAllWindows()

if len(objpoints) > 0:
    # Perform camera calibration
    ret, mtx, dist, rvecs, tvecs = cv2.calibrateCamera(
        objpoints, imgpoints, gray.shape[::-1], None, None
    )

    # Print out the camera calibration results
    print("Camera matrix : \n")
    print(mtx)
    print("dist : \n")
    print(dist)
    print("rvecs : \n")
    print(rvecs)
    print("tvecs : \n")
    print(tvecs)

    while True:
        ret, frame = cap.read()
        if not ret:
            break
        k = cv2.waitKey(1)

        if k % 256 == 27:
            # ESC pressed
            print("Escape hit, closing...")
            break

        # Convert to grayscale
        gray = cv2.cvtColor(frame, cv2.COLOR_BGR2GRAY)

        # Find the chess board corners
        ret, corners = cv2.findChessboardCorners(
            gray, (CHESSBOARD_Y, CHESSBOARD_X), None
        )

        if ret:
            corners2 = cv2.cornerSubPix(gray, corners, (11, 11), (-1, -1), criteria)
            imgpoints.append(corners2)

            # Draw and display the corners
            frame = cv2.drawChessboardCorners(
                frame, (CHESSBOARD_Y, CHESSBOARD_X), corners2, ret
            )

            # Estimate pose of pattern
            _, rvecs, tvecs, _ = cv2.solvePnPRansac(objp, corners2, mtx, dist)

            # Compute distance from camera to pattern
            x_distance = tvecs[0][0]
            y_distance = tvecs[1][0]
            z_distance = tvecs[2][0]

            text = f"X: {x_distance:.2f}m, Y: {y_distance:.2f}m, Z: {z_distance:.2f}m"
            cv2.putText(
                frame,
                text,
                (10, 30),
                cv2.FONT_HERSHEY_SIMPLEX,
                1,
                (0, 255, 0),
                2,
                cv2.LINE_AA,
            )

        cv2.imshow("Result", frame)

else:
    print("Not enough images where corners were found. Please capture more images.")

cap.release()
cv2.destroyAllWindows()
