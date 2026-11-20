/// compile: true

#let portique = <portique>
#let ouvrir = <ouvrir>
#let f(..args) = none
#f(
  f(name: portique),
  f(name: ouvrir),
  f(<adherent>, <portique>),
  f(name: <adherent>),
)
