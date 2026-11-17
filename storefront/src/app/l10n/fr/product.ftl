# A product's page: its price, the options to pick, its facts, what shoppers
# said of it and what goes with it.

product_rating = { NUMBER($rating, minimumFractionDigits: 1, maximumFractionDigits: 1) }/5 · { $count } avis
product_sold_out = Épuisé
product_few_left = Plus que { $left } en stock
product_quantity = Quantité
    .label = quantité de { $name }
product_delivery_within = Livraison avec suivi en { $days }
product_delivery = Livraison avec suivi
product_money_back = 14 jours satisfait ou remboursé
product_service = Un service client à votre écoute
product_variants = Tarifs par option
product_variants_price = Prix
product_variants_stock = Stock
product_variant_few_left = Plus que { $left }
product_variant_in_stock = En stock
product_description = Description
product_facts = Informations complémentaires
product_reviews = Avis de nos clients
    .pages = Pages d'avis
product_reviews_none = Aucun avis pour le moment. Vous l'avez acheté ? Donnez votre avis depuis la page de votre commande.
product_review_photo = Une photo du client
product_review_reply = Réponse de la boutique
product_reviews_newer = Avis plus récents
product_reviews_page = Page { $page } sur { $pages }
product_reviews_older = Avis plus anciens
product_related = Produits similaires
