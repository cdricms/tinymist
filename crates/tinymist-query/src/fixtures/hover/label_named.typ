/// compile: true

#let cas(..args) = none
#let f(..args) = none
#f(
  cas((1, 0), [Passer
    le portique], name: <portique>),
  f(<adherent>, /* position after */ <portique>),
)
