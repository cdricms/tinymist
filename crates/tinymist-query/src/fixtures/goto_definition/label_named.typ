/// compile: true

#let f(..args) = none
#f(
  f(name: <portique>),
  f(<adherent>, /* position after */ <portique>),
)
