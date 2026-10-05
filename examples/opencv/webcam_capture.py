import os

import cv2
import requests


def download_haarcascade(filename):
    url = f"https://raw.githubusercontent.com/opencv/opencv/master/data/haarcascades/{filename}"
    response = requests.get(url)
    response.raise_for_status()  # Check if the request was successful
    with open(filename, "wb") as f:
        f.write(response.content)


def capture_and_grayscale():
    filename = "haarcascade_frontalface_default.xml"

    if not os.path.isfile(filename):
        print(f"{filename} not found, downloading...")
        download_haarcascade(filename)

    # Load the cascade for face detection
    face_cascade = cv2.CascadeClassifier(filename)

    # Search for available webcams
    working_cam = None
    for index in range(3):
        cap = cv2.VideoCapture(index)
        if not cap.read()[0]:
            cap.release()
            continue
        else:
            working_cam = cap
            break

    # Check if the webcam is opened correctly
    if not working_cam.isOpened():
        raise OSError("Cannot open webcam")

    while True:
        # Read the current frame from the webcam
        ret, frame = working_cam.read()

        if not ret:
            break

        # Convert the image to grayscale
        gray = cv2.cvtColor(frame, cv2.COLOR_BGR2GRAY)

        # Detect faces
        faces = face_cascade.detectMultiScale(gray, 1.1, 4)

        # Convert grayscale image back to BGR
        gray_bgr = cv2.cvtColor(gray, cv2.COLOR_GRAY2BGR)

        # Replace the grayscale face area with the original colored face
        for x, y, w, h in faces:
            gray_bgr[y : y + h, x : x + w] = frame[y : y + h, x : x + w]
            cv2.rectangle(gray_bgr, (x, y), (x + w, y + h), (0, 255, 0), 2)

        # Display the image
        cv2.imshow("Input", gray_bgr)

        # Break the loop if 'q' is pressed
        if cv2.waitKey(1) & 0xFF == ord("q"):
            break

    # Release the VideoCapture object
    working_cam.release()

    # Destroy all windows
    cv2.destroyAllWindows()


if __name__ == "__main__":
    capture_and_grayscale()
