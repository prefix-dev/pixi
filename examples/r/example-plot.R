# Load the ggplot2 package
library(ggplot2)

# Load the built-in 'mtcars' dataset
data <- mtcars

# Create a scatterplot of 'mpg' vs 'wt'
ggplot(data, aes(x = wt, y = mpg)) +
  geom_point() +
  labs(x = "Weight (1000 lbs)", y = "Miles per Gallon") +
  ggtitle("Fuel Efficiency vs. Weight")

