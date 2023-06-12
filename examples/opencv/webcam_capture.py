import cv2
import os
import requests

def download_haarcascade(filename):
    url = f"https://raw.githubusercontent.com/opencv/opencv/master/data/haarcascades/{filename}"
    response = requests.get(url)
    response.raise_for_status()    # Check if the request was successful
    with open(filename, 'wb') as f:
        f.write(response.content)

def capture_and_grayscale():
    filename = 'haarcascade_frontalface_default.xml'

    if not os.path.isfile(filename):
        print(f"{filename} not found, downloading...")
        download_haarcascade(filename)

    # Load the cascade for face detection
    face_cascade = cv2.CascadeClassifier(filename)

    # Connect to the webcam
    cap = cv2.VideoCapture(0)

    # Check if the webcam is opened correctly
    if not cap.isOpened():
        raise IOError("Cannot open webcam")

    while True:
        # Read the current frame from the webcam
        ret, frame = cap.read()

        if not ret:
            break

        # Convert the image to grayscale
        gray = cv2.cvtColor(frame, cv2.COLOR_BGR2GRAY)

        # Detect faces
        faces = face_cascade.detectMultiScale(gray, 1.1, 4)

        # Convert grayscale image back to BGR
        gray_bgr = cv2.cvtColor(gray, cv2.COLOR_GRAY2BGR)

        # Replace the grayscale face area with the original colored face
        for (x, y, w, h) in faces:
            gray_bgr[y:y+h, x:x+w] = frame[y:y+h, x:x+w]
            cv2.rectangle(gray_bgr, (x, y), (x+w, y+h), (0, 255, 0), 2)

        # Display the image
        cv2.imshow('Input', gray_bgr)

        # Break the loop if 'q' is pressed
        if cv2.waitKey(1) & 0xFF == ord('q'):
            break

    # Release the VideoCapture object
    cap.release()

    # Destroy all windows
    cv2.destroyAllWindows()

if __name__ == '__main__':
    capture_and_grayscale()
