import pandas as pd
from sklearn.model_selection import train_test_split
import lightgbm as lgb
from sklearn.metrics import accuracy_score, confusion_matrix

# load data
df = pd.read_csv("Breast_cancer_data.csv")

# Declare feature vector and target variable
X = df[[
    'mean_radius','mean_texture','mean_perimeter',
    'mean_area','mean_smoothness']]
y = df['diagnosis']

# split the dataset into the training set and test set
X_train, X_test, y_train, y_test = train_test_split(
    X, y, test_size=0.3, random_state=42)

# build the lightgbm model
clf = lgb.LGBMClassifier(verbose=-1)
clf.fit(X_train, y_train)

# predict the results
y_pred = clf.predict(X_test)

# view accuracy
accuracy = accuracy_score(y_pred, y_test)
print(f"Model accuracy: {accuracy:0.3f}")

# view confusion-matrix
cm = confusion_matrix(y_test, y_pred)
print("True Positives(TP) = ", cm[0,0])
print("True Negatives(TN) = ", cm[1,1])
print("False Positives(FP) = ", cm[0,1])
print("False Negatives(FN) = ", cm[1,0])
