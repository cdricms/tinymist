/// contains: portique, passages

#let f(..args) = none
#f(
  f(name: <portique>),
  f(name: <passages>),
  f(<passages>, <portique>),
  f(<p>) /* range -3..-2 */
)
