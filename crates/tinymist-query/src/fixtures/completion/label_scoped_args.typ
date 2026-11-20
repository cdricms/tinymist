/// contains: adherent, portique, passages, reserver, annuler

#let reserver = <reserver>
#{
  let annuler = <annuler>
  diagram(
    node((0, 0), [A], name: <portique>),
    node((1, 0), [B], name: <passages>),
    edge(<adherent>, <p /* range -2..-1 */
