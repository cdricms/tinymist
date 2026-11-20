/// compile: true

#let portique = <portique>
#let f(..args) = none
#f(
  f(name: portique),
  f(/* position after */ <portique>),
)
